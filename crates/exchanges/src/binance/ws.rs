use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use cripton_core::{Exchange, OrderBook, PriceLevel, TradingPair};

use crate::traits::MarketEvent;

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
            Some(PriceLevel {
                price: price.parse().ok()?,
                quantity: qty.parse().ok()?,
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

pub async fn subscribe_orderbook(
    _client: &reqwest::Client,
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

    info!("Connected to Binance WebSocket, subscribed to {} pairs", pairs.len());

    tokio::spawn(async move {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(wrapper) = serde_json::from_str::<serde_json::Value>(&text.to_string()) {
                        if let Some(data) = wrapper.get("data") {
                            if let Ok(update) = serde_json::from_value::<WsDepthUpdate>(data.clone()) {
                                if let Some(pair) = symbol_to_pair(&update.symbol) {
                                    let orderbook = OrderBook {
                                        exchange: Exchange::Binance,
                                        pair,
                                        bids: parse_levels(&update.bids),
                                        asks: parse_levels(&update.asks),
                                        timestamp: Utc::now(),
                                    };
                                    if tx.send(MarketEvent::OrderBookUpdate(orderbook)).is_err() {
                                        warn!("Market event receiver dropped, stopping WS");
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(Message::Ping(_)) => {}
                Ok(Message::Close(_)) => {
                    warn!("Binance WebSocket closed");
                    let _ = tx.send(MarketEvent::ConnectionLost(Exchange::Binance));
                    break;
                }
                Err(e) => {
                    error!("Binance WebSocket error: {}", e);
                    let _ = tx.send(MarketEvent::ConnectionLost(Exchange::Binance));
                    break;
                }
                _ => {}
            }
        }
    });

    Ok(())
}
