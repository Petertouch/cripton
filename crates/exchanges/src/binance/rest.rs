use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio::sync::mpsc;

use cripton_core::{Exchange, OrderBook, PriceLevel, Side, Ticker, TradingPair};

use super::ws;

use crate::traits::{ExchangeConnector, MarketEvent};

const BASE_URL: &str = "https://api.binance.com";

pub struct BinanceClient {
    client: Client,
    api_key: String,
    api_secret: String,
}

impl BinanceClient {
    pub fn new(api_key: String, api_secret: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_secret,
        }
    }

    fn symbol(pair: TradingPair) -> Result<String> {
        pair.as_binance_symbol()
            .map(|s| s.to_string())
            .context(format!("Pair {} not supported on Binance", pair))
    }
}

// --- Binance API response types ---

#[derive(Debug, Deserialize)]
struct BinanceOrderBookResponse {
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTickerResponse {
    symbol: String,
    bid_price: String,
    ask_price: String,
    last_price: String,
    volume: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceOrderResponse {
    order_id: u64,
}

#[derive(Debug, Deserialize)]
struct BinanceBalanceInfo {
    asset: String,
    free: String,
}

#[derive(Debug, Deserialize)]
struct BinanceAccountResponse {
    balances: Vec<BinanceBalanceInfo>,
}

fn parse_price_levels(raw: &[[String; 2]]) -> Vec<PriceLevel> {
    raw.iter()
        .filter_map(|[price, qty]| {
            Some(PriceLevel {
                price: price.parse().ok()?,
                quantity: qty.parse().ok()?,
            })
        })
        .collect()
}

#[async_trait]
impl ExchangeConnector for BinanceClient {
    fn exchange(&self) -> Exchange {
        Exchange::Binance
    }

    async fn fetch_orderbook(&self, pair: TradingPair) -> Result<OrderBook> {
        let symbol = Self::symbol(pair)?;
        let url = format!("{}/api/v3/depth?symbol={}&limit=20", BASE_URL, symbol);

        let resp: BinanceOrderBookResponse = self
            .client
            .get(&url)
            .send()
            .await?
            .json()
            .await?;

        Ok(OrderBook {
            exchange: Exchange::Binance,
            pair,
            bids: parse_price_levels(&resp.bids),
            asks: parse_price_levels(&resp.asks),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_ticker(&self, pair: TradingPair) -> Result<Ticker> {
        let symbol = Self::symbol(pair)?;
        let url = format!(
            "{}/api/v3/ticker/bookTicker?symbol={}",
            BASE_URL, symbol
        );

        let resp: BinanceTickerResponse = self
            .client
            .get(&url)
            .send()
            .await?
            .json()
            .await?;

        Ok(Ticker {
            exchange: Exchange::Binance,
            pair,
            bid: resp.bid_price.parse()?,
            ask: resp.ask_price.parse()?,
            last_price: resp.last_price.parse().unwrap_or_default(),
            volume_24h: resp.volume.parse().unwrap_or_default(),
            timestamp: Utc::now(),
        })
    }

    async fn subscribe_orderbook(
        &self,
        pairs: &[TradingPair],
        tx: mpsc::UnboundedSender<MarketEvent>,
    ) -> Result<()> {
        ws::subscribe_orderbook(&self.client, pairs, tx).await
    }

    async fn place_limit_order(
        &self,
        pair: TradingPair,
        side: Side,
        price: Decimal,
        quantity: Decimal,
    ) -> Result<String> {
        let symbol = Self::symbol(pair)?;
        let side_str = match side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };

        let timestamp = Utc::now().timestamp_millis();
        let query = format!(
            "symbol={}&side={}&type=LIMIT&timeInForce=GTC&price={}&quantity={}&timestamp={}",
            symbol, side_str, price, quantity, timestamp
        );
        let signature = self.sign(&query);

        let url = format!(
            "{}/api/v3/order?{}&signature={}",
            BASE_URL, query, signature
        );

        let resp: BinanceOrderResponse = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.order_id.to_string())
    }

    async fn place_market_order(
        &self,
        pair: TradingPair,
        side: Side,
        quantity: Decimal,
    ) -> Result<String> {
        let symbol = Self::symbol(pair)?;
        let side_str = match side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };

        let timestamp = Utc::now().timestamp_millis();
        let query = format!(
            "symbol={}&side={}&type=MARKET&quantity={}&timestamp={}",
            symbol, side_str, quantity, timestamp
        );
        let signature = self.sign(&query);

        let url = format!(
            "{}/api/v3/order?{}&signature={}",
            BASE_URL, query, signature
        );

        let resp: BinanceOrderResponse = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.order_id.to_string())
    }

    async fn cancel_order(&self, pair: TradingPair, order_id: &str) -> Result<()> {
        let symbol = Self::symbol(pair)?;
        let timestamp = Utc::now().timestamp_millis();
        let query = format!(
            "symbol={}&orderId={}&timestamp={}",
            symbol, order_id, timestamp
        );
        let signature = self.sign(&query);

        let url = format!(
            "{}/api/v3/order?{}&signature={}",
            BASE_URL, query, signature
        );

        self.client
            .delete(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?;

        Ok(())
    }

    async fn get_balance(&self, asset: &str) -> Result<Decimal> {
        let timestamp = Utc::now().timestamp_millis();
        let query = format!("timestamp={}", timestamp);
        let signature = self.sign(&query);

        let url = format!(
            "{}/api/v3/account?{}&signature={}",
            BASE_URL, query, signature
        );

        let resp: BinanceAccountResponse = self
            .client
            .get(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .json()
            .await?;

        let balance = resp
            .balances
            .iter()
            .find(|b| b.asset == asset)
            .map(|b| b.free.parse::<Decimal>().unwrap_or_default())
            .unwrap_or_default();

        Ok(balance)
    }
}

impl BinanceClient {
    fn sign(&self, query: &str) -> String {
        use std::fmt::Write;
        let key = hmac_sha256::HMAC::mac(query.as_bytes(), self.api_secret.as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in key {
            write!(hex, "{:02x}", byte).unwrap();
        }
        hex
    }
}
