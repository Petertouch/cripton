use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, StatusCode};
use rust_decimal::Decimal;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::debug;
use zeroize::Zeroize;

use cripton_core::{Exchange, OrderBook, PriceLevel, Side, Ticker, TradingPair};

use super::ws;

use crate::traits::{ExchangeConnector, MarketEvent};

const BASE_URL: &str = "https://api.binance.com";
const RECV_WINDOW: u64 = 5000; // 5s validity window for signed requests
const HTTP_TIMEOUT_SECS: u64 = 10;
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 5;

pub struct BinanceClient {
    client: Client,
    api_key: SecretString,
    api_secret: SecretString,
}

// Custom Debug to never leak secrets
impl fmt::Debug for BinanceClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BinanceClient")
            .field("api_key", &"[REDACTED]")
            .field("api_secret", &"[REDACTED]")
            .finish()
    }
}

impl BinanceClient {
    /// SEC: takes ownership of credential strings, wraps in SecretString,
    /// then zeroizes the originals. No cloning — credentials exist in
    /// exactly one place (SecretString) after construction.
    pub fn new(mut api_key: String, mut api_secret: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
            .pool_max_idle_per_host(5)
            .https_only(true)
            .build()
            .expect("Failed to build HTTP client");

        // SEC: SecretString::from() for String takes ownership via Into<Box<str>>,
        // so we must create it before zeroizing. The original String's heap
        // allocation is consumed by SecretString — no copy is made.
        let key_secret = SecretString::from(std::mem::take(&mut api_key));
        let secret_secret = SecretString::from(std::mem::take(&mut api_secret));

        // SEC: zeroize the now-empty originals (clears any inline capacity)
        api_key.zeroize();
        api_secret.zeroize();

        Self {
            client,
            api_key: key_secret,
            api_secret: secret_secret,
        }
    }

    fn symbol(pair: TradingPair) -> Result<String> {
        pair.as_binance_symbol()
            .map(|s| s.to_string())
            .context(format!("Pair {} not supported on Binance", pair))
    }

    /// Send a signed POST request. Signature goes in the request body, NOT the URL.
    async fn signed_post(&self, endpoint: &str, query: &str) -> Result<String> {
        let signature = self.sign(query);
        let body = format!("{}&signature={}", query, signature);
        let url = format!("{}{}", BASE_URL, endpoint);

        let resp = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", self.api_key.expose_secret())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .context("HTTP request failed")?;

        Self::validate_response(resp).await
    }

    /// Send a signed GET request.
    async fn signed_get(&self, endpoint: &str, query: &str) -> Result<String> {
        let signature = self.sign(query);
        let url = format!("{}{}?{}&signature={}", BASE_URL, endpoint, query, signature);

        let resp = self
            .client
            .get(&url)
            .header("X-MBX-APIKEY", self.api_key.expose_secret())
            .send()
            .await
            .context("HTTP request failed")?;

        Self::validate_response(resp).await
    }

    /// Send a signed DELETE request.
    async fn signed_delete(&self, endpoint: &str, query: &str) -> Result<()> {
        let signature = self.sign(query);
        let body = format!("{}&signature={}", query, signature);
        let url = format!("{}{}", BASE_URL, endpoint);

        let resp = self
            .client
            .delete(&url)
            .header("X-MBX-APIKEY", self.api_key.expose_secret())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .context("HTTP request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Binance API error (HTTP {}): {}", status.as_u16(), Self::sanitize_error(&body));
        }

        Ok(())
    }

    /// Validate HTTP response status before parsing JSON
    async fn validate_response(resp: reqwest::Response) -> Result<String> {
        let status = resp.status();

        if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::IM_A_TEAPOT {
            anyhow::bail!("Rate limited by Binance (HTTP {})", status.as_u16());
        }

        let body = resp.text().await.context("Failed to read response body")?;

        if !status.is_success() {
            anyhow::bail!(
                "Binance API error (HTTP {}): {}",
                status.as_u16(),
                Self::sanitize_error(&body)
            );
        }

        Ok(body)
    }

    /// Remove sensitive data from error messages before logging
    fn sanitize_error(body: &str) -> String {
        // Binance errors are JSON: {"code":-1021,"msg":"Timestamp for this request..."}
        // Only extract code and msg, never echo back full request params
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("unknown");
            format!("code={}, msg={}", code, msg)
        } else {
            "non-JSON error response".to_string()
        }
    }

    fn sign(&self, query: &str) -> String {
        use std::fmt::Write;
        let key = hmac_sha256::HMAC::mac(
            query.as_bytes(),
            self.api_secret.expose_secret().as_bytes(),
        );
        let mut hex = String::with_capacity(64);
        for byte in key {
            write!(hex, "{:02x}", byte).unwrap();
        }
        // key is stack-allocated [u8; 32], dropped here automatically
        hex
    }
}

// --- Binance API response types ---

#[derive(Deserialize)]
struct BinanceOrderBookResponse {
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTickerResponse {
    bid_price: String,
    ask_price: String,
    last_price: String,
    volume: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceOrderResponse {
    order_id: u64,
}

#[derive(Deserialize)]
struct BinanceBalanceInfo {
    asset: String,
    free: String,
}

#[derive(Deserialize)]
struct BinanceAccountResponse {
    balances: Vec<BinanceBalanceInfo>,
}

fn parse_price_levels(raw: &[[String; 2]]) -> Vec<PriceLevel> {
    raw.iter()
        .filter_map(|[price, qty]| {
            let p: Decimal = price.parse().ok()?;
            let q: Decimal = qty.parse().ok()?;
            // SEC: reject negative or zero prices/quantities
            if p <= Decimal::ZERO || q < Decimal::ZERO {
                return None;
            }
            Some(PriceLevel { price: p, quantity: q })
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

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch order book")?;

        let body = Self::validate_response(resp).await?;
        let data: BinanceOrderBookResponse = serde_json::from_str(&body)
            .context("Failed to parse order book response")?;

        let mut bids = parse_price_levels(&data.bids);
        let mut asks = parse_price_levels(&data.asks);

        // SEC: ensure proper sort order
        bids.sort_by(|a, b| b.price.cmp(&a.price)); // descending
        asks.sort_by(|a, b| a.price.cmp(&b.price)); // ascending

        debug!("{} {} orderbook: {} bids, {} asks", Exchange::Binance, pair, bids.len(), asks.len());

        Ok(OrderBook {
            exchange: Exchange::Binance,
            pair,
            bids,
            asks,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_ticker(&self, pair: TradingPair) -> Result<Ticker> {
        let symbol = Self::symbol(pair)?;
        let url = format!("{}/api/v3/ticker/bookTicker?symbol={}", BASE_URL, symbol);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch ticker")?;

        let body = Self::validate_response(resp).await?;
        let data: BinanceTickerResponse = serde_json::from_str(&body)
            .context("Failed to parse ticker response")?;

        Ok(Ticker {
            exchange: Exchange::Binance,
            pair,
            bid: data.bid_price.parse()?,
            ask: data.ask_price.parse()?,
            last_price: data.last_price.parse().unwrap_or_default(),
            volume_24h: data.volume.parse().unwrap_or_default(),
            timestamp: Utc::now(),
        })
    }

    async fn subscribe_orderbook(
        &self,
        pairs: &[TradingPair],
        tx: mpsc::UnboundedSender<MarketEvent>,
    ) -> Result<()> {
        ws::subscribe_orderbook(pairs, tx).await
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
            "symbol={}&side={}&type=LIMIT&timeInForce=GTC&price={}&quantity={}&recvWindow={}&timestamp={}",
            symbol, side_str, price, quantity, RECV_WINDOW, timestamp
        );

        let body = self.signed_post("/api/v3/order", &query).await?;
        let resp: BinanceOrderResponse = serde_json::from_str(&body)
            .context("Failed to parse order response")?;

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
            "symbol={}&side={}&type=MARKET&quantity={}&recvWindow={}&timestamp={}",
            symbol, side_str, quantity, RECV_WINDOW, timestamp
        );

        let body = self.signed_post("/api/v3/order", &query).await?;
        let resp: BinanceOrderResponse = serde_json::from_str(&body)
            .context("Failed to parse order response")?;

        Ok(resp.order_id.to_string())
    }

    async fn cancel_order(&self, pair: TradingPair, order_id: &str) -> Result<()> {
        // SEC: validate order_id is numeric only to prevent parameter injection
        if !order_id.chars().all(|c| c.is_ascii_digit()) {
            anyhow::bail!("Invalid order ID format");
        }

        let symbol = Self::symbol(pair)?;
        let timestamp = Utc::now().timestamp_millis();
        let query = format!(
            "symbol={}&orderId={}&recvWindow={}&timestamp={}",
            symbol, order_id, RECV_WINDOW, timestamp
        );

        self.signed_delete("/api/v3/order", &query).await
    }

    async fn get_balance(&self, asset: &str) -> Result<Decimal> {
        // SEC: validate asset is alphanumeric only
        if !asset.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("Invalid asset identifier");
        }

        let timestamp = Utc::now().timestamp_millis();
        let query = format!("recvWindow={}&timestamp={}", RECV_WINDOW, timestamp);

        let body = self.signed_get("/api/v3/account", &query).await?;
        let resp: BinanceAccountResponse = serde_json::from_str(&body)
            .context("Failed to parse account response")?;

        let balance = resp
            .balances
            .iter()
            .find(|b| b.asset == asset)
            .map(|b| b.free.parse::<Decimal>().unwrap_or_default())
            .unwrap_or_default();

        Ok(balance)
    }
}
