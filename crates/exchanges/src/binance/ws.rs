use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use cripton_core::{Exchange, OrderBook, PriceLevel, TradingPair};

use crate::traits::MarketEvent;

const MAX_RECONNECT_ATTEMPTS: u32 = 10;
const INITIAL_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 30_000;

#[derive(Debug, Deserialize)]
struct WsDepthUpdate {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "b")]
    bids: Vec<[String; 2]>,
    #[serde(rename = "a")]
    asks: Vec<[String; 2]>,
}

fn parse_levels(raw: &[[String; 2]]) -> Vec<PriceLevel> {
    raw.iter()
        .filter_map(|[price, qty]| {
            let p: Decimal = price.parse().ok()?;
            let q: Decimal = qty.parse().ok()?;
            // SEC: reject negative or zero prices
            if p <= Decimal::ZERO || q < Decimal::ZERO {
                warn!("WS: rejected invalid price level: price={}, qty={}", p, q);
                return None;
            }
            Some(PriceLevel {
                price: p,
                quantity: q,
            })
        })
        .collect()
}

fn symbol_to_pair(symbol: &str) -> Option<TradingPair> {
    match symbol {
        "USDTUSDC" => Some(TradingPair::UsdtUsdc),
        "EURCUSDT" => Some(TradingPair::UsdtEurc),
        "EURCUSDC" => Some(TradingPair::EurcUsdc),
        "EURUSDT" => Some(TradingPair::EurUsdt),
        "EURUSDC" => Some(TradingPair::EurUsdc),
        _ => None,
    }
}

/// Validate that an order book has sane data
fn validate_orderbook(book: &OrderBook) -> bool {
    // Must have at least one bid and one ask
    if book.bids.is_empty() || book.asks.is_empty() {
        return false;
    }

    // Bids must be sorted descending, asks ascending
    let bids_sorted = book.bids.windows(2).all(|w| w[0].price >= w[1].price);
    let asks_sorted = book.asks.windows(2).all(|w| w[0].price <= w[1].price);

    if !bids_sorted || !asks_sorted {
        warn!(
            "WS: order book not properly sorted for {} {}",
            book.exchange, book.pair
        );
        return false;
    }

    // Best bid must be less than best ask (no crossed book)
    if let (Some(bid), Some(ask)) = (book.best_bid(), book.best_ask())
        && bid.price >= ask.price
    {
        warn!(
            "WS: crossed order book for {} {}: bid={} >= ask={}",
            book.exchange, book.pair, bid.price, ask.price
        );
        return false;
    }

    true
}

pub async fn subscribe_orderbook(
    pairs: &[TradingPair],
    tx: mpsc::UnboundedSender<MarketEvent>,
) -> Result<()> {
    let streams: Vec<String> = pairs
        .iter()
        .filter_map(|p| {
            p.as_binance_symbol()
                .map(|s| format!("{}@depth20@100ms", s.to_lowercase()))
        })
        .collect();

    if streams.is_empty() {
        anyhow::bail!("No valid Binance pairs to subscribe to");
    }

    let url = format!(
        "wss://stream.binance.com:9443/stream?streams={}",
        streams.join("/")
    );

    info!("Connecting to Binance WebSocket: {}", url);

    let (ws_stream, _) = connect_async(&url)
        .await
        .context("Failed to connect to Binance WebSocket")?;

    let (_write, mut read) = ws_stream.split();

    info!(
        "Connected to Binance WebSocket, subscribed to {} pairs",
        pairs.len()
    );

    // Clone what we need for the reconnection loop
    let url_for_reconnect = url.clone();
    let tx_clone = tx.clone();

    tokio::spawn(async move {
        let mut reconnect_attempts: u32 = 0;

        loop {
            // Process messages from current connection
            let disconnected = process_messages(&mut read, &tx_clone).await;

            if !disconnected {
                // Channel closed by receiver, exit cleanly
                break;
            }

            // Attempt reconnection with exponential backoff
            reconnect_attempts = reconnect_attempts.saturating_add(1);
            if reconnect_attempts > MAX_RECONNECT_ATTEMPTS {
                error!(
                    "WS: exceeded {} reconnection attempts, giving up",
                    MAX_RECONNECT_ATTEMPTS
                );
                let _ = tx_clone.send(MarketEvent::ConnectionLost(Exchange::Binance));
                break;
            }

            let backoff = std::cmp::min(
                INITIAL_BACKOFF_MS * 2u64.saturating_pow(reconnect_attempts - 1),
                MAX_BACKOFF_MS,
            );
            warn!(
                "WS: reconnecting in {}ms (attempt {}/{})",
                backoff, reconnect_attempts, MAX_RECONNECT_ATTEMPTS
            );

            tokio::time::sleep(Duration::from_millis(backoff)).await;

            match connect_async(&url_for_reconnect).await {
                Ok((new_ws, _)) => {
                    let (_new_write, new_read) = new_ws.split();
                    read = new_read;
                    reconnect_attempts = 0;
                    info!("WS: reconnected to Binance");
                    let _ = tx_clone.send(MarketEvent::ConnectionRestored(Exchange::Binance));
                }
                Err(e) => {
                    error!("WS: reconnection failed: {}", e);
                }
            }
        }
    });

    Ok(())
}

/// Parse a single WebSocket text message into an OrderBook
fn parse_ws_message(text: &str) -> Option<OrderBook> {
    let wrapper: serde_json::Value = serde_json::from_str(text).ok()?;
    let data = wrapper.get("data")?;
    let update: WsDepthUpdate = serde_json::from_value(data.clone()).ok()?;
    let pair = symbol_to_pair(&update.symbol)?;

    let mut bids = parse_levels(&update.bids);
    let mut asks = parse_levels(&update.asks);

    bids.sort_by(|a, b| b.price.cmp(&a.price));
    asks.sort_by(|a, b| a.price.cmp(&b.price));

    Some(OrderBook {
        exchange: Exchange::Binance,
        pair,
        bids,
        asks,
        timestamp: Utc::now(),
    })
}

/// Process incoming WebSocket messages. Returns true if disconnected (should reconnect),
/// false if the channel was closed (should exit).
async fn process_messages(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    tx: &mpsc::UnboundedSender<MarketEvent>,
) -> bool {
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let Some(orderbook) = parse_ws_message(text.as_ref()) else {
                    continue;
                };

                if !validate_orderbook(&orderbook) {
                    continue;
                }

                if tx.send(MarketEvent::OrderBookUpdate(orderbook)).is_err() {
                    return false;
                }
            }
            Ok(Message::Ping(_)) => {}
            Ok(Message::Close(_)) => {
                warn!("WS: Binance closed connection");
                return true; // reconnect
            }
            Err(e) => {
                error!("WS: error: {}", e);
                return true; // reconnect
            }
            _ => {}
        }
    }

    // Stream ended
    true
}
