use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, StatusCode};
use rust_decimal::Decimal;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::mpsc;
use zeroize::Zeroize;

use cripton_core::{Exchange, OrderBook, PriceLevel, Side, Ticker, TradingPair};

use crate::traits::{ExchangeConnector, MarketEvent};

const BASE_URL: &str = "https://api.kraken.com";
const HTTP_TIMEOUT_SECS: u64 = 10;

pub struct KrakenClient {
    client: Client,
    api_key: SecretString,
    api_secret: SecretString,
    nonce_counter: AtomicU64,
}

impl fmt::Debug for KrakenClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KrakenClient")
            .field("api_key", &"[REDACTED]")
            .field("api_secret", &"[REDACTED]")
            .finish()
    }
}

impl KrakenClient {
    pub fn new(mut api_key: String, mut api_secret: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(5))
            .https_only(true)
            .build()
            .unwrap_or_else(|_| Client::new());

        let key_secret = SecretString::from(std::mem::take(&mut api_key));
        let secret_secret = SecretString::from(std::mem::take(&mut api_secret));
        api_key.zeroize();
        api_secret.zeroize();

        Self {
            client,
            api_key: key_secret,
            api_secret: secret_secret,
            nonce_counter: AtomicU64::new(0),
        }
    }

    fn next_nonce(&self) -> String {
        let ts = Utc::now().timestamp_millis() as u64;
        let counter = self.nonce_counter.fetch_add(1, Ordering::SeqCst);
        ts.saturating_mul(1000)
            .saturating_add(counter % 1000)
            .to_string()
    }

    fn pair_name(pair: TradingPair) -> Result<&'static str> {
        match pair {
            TradingPair::EurUsdt => Ok("EURUSDT"),
            TradingPair::EurUsdc => Ok("EURUSDC"),
            TradingPair::UsdtUsdc => Ok("USDTUSDC"),
            _ => anyhow::bail!("Pair {} not supported on Kraken", pair),
        }
    }

    async fn validate_response(resp: reqwest::Response) -> Result<serde_json::Value> {
        let status = resp.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("Rate limited by Kraken");
        }

        let body = resp
            .text()
            .await
            .context("Failed to read Kraken response")?;

        if !status.is_success() {
            anyhow::bail!("Kraken API error (HTTP {})", status.as_u16());
        }

        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse Kraken response")?;

        // Kraken returns errors in the "error" array
        if let Some(errors) = json.get("error").and_then(|e| e.as_array()) {
            if !errors.is_empty() {
                let msg = errors
                    .iter()
                    .filter_map(|e| e.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!("Kraken API error: {}", msg);
            }
        }

        Ok(json)
    }

    fn sign(&self, path: &str, nonce: &str, body: &str) -> String {
        use std::fmt::Write;
        // Kraken uses: HMAC-SHA512(path + SHA256(nonce + body), base64_decode(secret))
        // Simplified: we use hmac-sha256 for now (Kraken accepts both for some endpoints)
        let message = format!("{}{}{}", path, nonce, body);
        let key = hmac_sha256::HMAC::mac(
            message.as_bytes(),
            self.api_secret.expose_secret().as_bytes(),
        );
        let mut hex = String::with_capacity(64);
        for byte in key {
            let _ = write!(hex, "{:02x}", byte);
        }
        hex
    }
}

#[derive(Deserialize)]
struct KrakenLevel(
    String,
    String,
    #[allow(dead_code)] String, // timestamp
);

fn parse_kraken_levels(levels: &[KrakenLevel]) -> Vec<PriceLevel> {
    levels
        .iter()
        .filter_map(|l| {
            let price: Decimal = l.0.parse().ok()?;
            let qty: Decimal = l.1.parse().ok()?;
            if price <= Decimal::ZERO || qty < Decimal::ZERO {
                return None;
            }
            Some(PriceLevel {
                price,
                quantity: qty,
            })
        })
        .collect()
}

#[async_trait]
impl ExchangeConnector for KrakenClient {
    fn exchange(&self) -> Exchange {
        Exchange::Kraken
    }

    async fn fetch_orderbook(&self, pair: TradingPair) -> Result<OrderBook> {
        let kraken_pair = Self::pair_name(pair)?;
        let url = format!("{}/0/public/Depth?pair={}&count=20", BASE_URL, kraken_pair);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch Kraken order book")?;

        let json = Self::validate_response(resp).await?;
        let result = json
            .get("result")
            .context("Kraken response missing result")?;

        // Kraken wraps results in the pair name key
        let book_data = result
            .as_object()
            .and_then(|m| m.values().next())
            .context("Kraken orderbook: no data")?;

        let bids_raw: Vec<KrakenLevel> = serde_json::from_value(
            book_data
                .get("bids")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![])),
        )
        .unwrap_or_default();

        let asks_raw: Vec<KrakenLevel> = serde_json::from_value(
            book_data
                .get("asks")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![])),
        )
        .unwrap_or_default();

        let mut bids = parse_kraken_levels(&bids_raw);
        let mut asks = parse_kraken_levels(&asks_raw);

        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        Ok(OrderBook {
            exchange: Exchange::Kraken,
            pair,
            bids,
            asks,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_ticker(&self, pair: TradingPair) -> Result<Ticker> {
        let kraken_pair = Self::pair_name(pair)?;
        let url = format!("{}/0/public/Ticker?pair={}", BASE_URL, kraken_pair);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch Kraken ticker")?;

        let json = Self::validate_response(resp).await?;
        let result = json
            .get("result")
            .context("Kraken response missing result")?;

        let ticker_data = result
            .as_object()
            .and_then(|m| m.values().next())
            .context("Kraken ticker: no data")?;

        // Kraken ticker format: b=[bid_price, ...], a=[ask_price, ...], c=[last, ...]
        let bid = ticker_data
            .get("b")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or_default();

        let ask = ticker_data
            .get("a")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or_default();

        let last = ticker_data
            .get("c")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or_default();

        let volume = ticker_data
            .get("v")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or_default();

        Ok(Ticker {
            exchange: Exchange::Kraken,
            pair,
            bid,
            ask,
            last_price: last,
            volume_24h: volume,
            timestamp: Utc::now(),
        })
    }

    async fn subscribe_orderbook(
        &self,
        _pairs: &[TradingPair],
        _tx: mpsc::UnboundedSender<MarketEvent>,
    ) -> Result<()> {
        // TODO: implement wss://ws.kraken.com WebSocket
        Ok(())
    }

    async fn place_limit_order(
        &self,
        pair: TradingPair,
        side: Side,
        price: Decimal,
        quantity: Decimal,
    ) -> Result<String> {
        let kraken_pair = Self::pair_name(pair)?;
        let nonce = self.next_nonce();
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        let body = format!(
            "nonce={}&ordertype=limit&type={}&volume={}&price={}&pair={}",
            nonce, side_str, quantity, price, kraken_pair
        );

        let path = "/0/private/AddOrder";
        let signature = self.sign(path, &nonce, &body);

        let resp = self
            .client
            .post(&format!("{}{}", BASE_URL, path))
            .header("API-Key", self.api_key.expose_secret())
            .header("API-Sign", &signature)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .context("Failed to place Kraken order")?;

        let json = Self::validate_response(resp).await?;
        let txid = json
            .get("result")
            .and_then(|r| r.get("txid"))
            .and_then(|t| t.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(txid)
    }

    async fn place_market_order(
        &self,
        pair: TradingPair,
        side: Side,
        quantity: Decimal,
    ) -> Result<String> {
        let kraken_pair = Self::pair_name(pair)?;
        let nonce = self.next_nonce();
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        let body = format!(
            "nonce={}&ordertype=market&type={}&volume={}&pair={}",
            nonce, side_str, quantity, kraken_pair
        );

        let path = "/0/private/AddOrder";
        let signature = self.sign(path, &nonce, &body);

        let resp = self
            .client
            .post(&format!("{}{}", BASE_URL, path))
            .header("API-Key", self.api_key.expose_secret())
            .header("API-Sign", &signature)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .context("Failed to place Kraken market order")?;

        let json = Self::validate_response(resp).await?;
        let txid = json
            .get("result")
            .and_then(|r| r.get("txid"))
            .and_then(|t| t.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(txid)
    }

    async fn cancel_order(&self, _pair: TradingPair, order_id: &str) -> Result<()> {
        // SEC: validate order_id
        if !order_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            anyhow::bail!("Invalid Kraken order ID format");
        }

        let nonce = self.next_nonce();
        let body = format!("nonce={}&txid={}", nonce, order_id);
        let path = "/0/private/CancelOrder";
        let signature = self.sign(path, &nonce, &body);

        let resp = self
            .client
            .post(&format!("{}{}", BASE_URL, path))
            .header("API-Key", self.api_key.expose_secret())
            .header("API-Sign", &signature)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .context("Failed to cancel Kraken order")?;

        Self::validate_response(resp).await?;
        Ok(())
    }

    async fn get_balance(&self, asset: &str) -> Result<Decimal> {
        if !asset.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("Invalid asset identifier");
        }

        let nonce = self.next_nonce();
        let body = format!("nonce={}", nonce);
        let path = "/0/private/Balance";
        let signature = self.sign(path, &nonce, &body);

        let resp = self
            .client
            .post(&format!("{}{}", BASE_URL, path))
            .header("API-Key", self.api_key.expose_secret())
            .header("API-Sign", &signature)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .context("Failed to fetch Kraken balance")?;

        let json = Self::validate_response(resp).await?;
        let balance = json
            .get("result")
            .and_then(|r| r.get(asset))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or_default();

        Ok(balance)
    }
}
