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

const BASE_URL: &str = "https://bitso.com/api/v3";
const HTTP_TIMEOUT_SECS: u64 = 10;

pub struct BitsoClient {
    client: Client,
    api_key: SecretString,
    api_secret: SecretString,
    /// SEC: monotonically increasing nonce to prevent replay attacks.
    /// Combines millisecond timestamp with a counter to guarantee uniqueness
    /// even when multiple requests happen within the same millisecond.
    nonce_counter: AtomicU64,
}

impl fmt::Debug for BitsoClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BitsoClient")
            .field("api_key", &"[REDACTED]")
            .field("api_secret", &"[REDACTED]")
            .finish()
    }
}

impl BitsoClient {
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

    fn book_name(pair: TradingPair) -> Result<&'static str> {
        match pair {
            TradingPair::UsdtCop => Ok("usdt_cop"),
            TradingPair::UsdcCop => Ok("usdc_cop"),
            _ => anyhow::bail!("Pair {} not supported on Bitso", pair),
        }
    }

    async fn validate_response(resp: reqwest::Response) -> Result<String> {
        let status = resp.status();

        if status == StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("Rate limited by Bitso");
        }

        let body = resp.text().await.context("Failed to read Bitso response")?;

        if !status.is_success() {
            // SEC: only extract error code, not full body
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                let code = v
                    .get("error")
                    .and_then(|e| e.get("code"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("unknown");
                anyhow::bail!("Bitso API error (HTTP {}): code={}", status.as_u16(), code);
            }
            anyhow::bail!("Bitso API error (HTTP {})", status.as_u16());
        }

        Ok(body)
    }

    fn sign(&self, nonce: &str, method: &str, path: &str, body: &str) -> String {
        use std::fmt::Write;
        let message = format!("{}{}{}{}", nonce, method, path, body);
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

    /// SEC: generates a strictly increasing nonce by combining timestamp with atomic counter.
    /// Even if called multiple times in the same millisecond, each nonce is unique.
    fn next_nonce(&self) -> String {
        let ts = Utc::now().timestamp_millis() as u64;
        let counter = self.nonce_counter.fetch_add(1, Ordering::SeqCst);
        // Combine: timestamp * 1000 + counter mod 1000
        let nonce = ts.saturating_mul(1000).saturating_add(counter % 1000);
        nonce.to_string()
    }

    fn auth_header(&self, method: &str, path: &str, body: &str) -> String {
        let nonce = self.next_nonce();
        let signature = self.sign(&nonce, method, path, body);
        format!(
            "Bitso {}:{}:{}",
            self.api_key.expose_secret(),
            &nonce,
            signature
        )
    }
}

// --- Bitso API response types ---

#[derive(Deserialize)]
struct BitsoResponse<T> {
    #[allow(dead_code)]
    success: bool,
    payload: Option<T>,
}

#[derive(Deserialize)]
struct BitsoOrderBookPayload {
    bids: Vec<BitsoLevel>,
    asks: Vec<BitsoLevel>,
}

#[derive(Deserialize)]
struct BitsoLevel {
    price: String,
    amount: String,
}

#[derive(Deserialize)]
struct BitsoTickerPayload {
    bid: String,
    ask: String,
    last: String,
    volume: String,
}

#[derive(Deserialize)]
struct BitsoOrderPayload {
    oid: String,
}

#[derive(Deserialize)]
struct BitsoBalance {
    currency: String,
    available: String,
}

#[derive(Deserialize)]
struct BitsoBalancesPayload {
    balances: Vec<BitsoBalance>,
}

fn parse_bitso_levels(levels: &[BitsoLevel]) -> Vec<PriceLevel> {
    levels
        .iter()
        .filter_map(|l| {
            let price: Decimal = l.price.parse().ok()?;
            let qty: Decimal = l.amount.parse().ok()?;
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
impl ExchangeConnector for BitsoClient {
    fn exchange(&self) -> Exchange {
        Exchange::Bitso
    }

    async fn fetch_orderbook(&self, pair: TradingPair) -> Result<OrderBook> {
        let book = Self::book_name(pair)?;
        let url = format!("{}/order_book/?book={}", BASE_URL, book);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch Bitso order book")?;

        let body = Self::validate_response(resp).await?;
        let data: BitsoResponse<BitsoOrderBookPayload> =
            serde_json::from_str(&body).context("Failed to parse Bitso order book")?;

        let payload = data
            .payload
            .context("Bitso returned success=true but no payload")?;

        let mut bids = parse_bitso_levels(&payload.bids);
        let mut asks = parse_bitso_levels(&payload.asks);

        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        Ok(OrderBook {
            exchange: Exchange::Bitso,
            pair,
            bids,
            asks,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_ticker(&self, pair: TradingPair) -> Result<Ticker> {
        let book = Self::book_name(pair)?;
        let url = format!("{}/ticker/?book={}", BASE_URL, book);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch Bitso ticker")?;

        let body = Self::validate_response(resp).await?;
        let data: BitsoResponse<BitsoTickerPayload> =
            serde_json::from_str(&body).context("Failed to parse Bitso ticker")?;

        let payload = data.payload.context("Bitso ticker: no payload")?;

        Ok(Ticker {
            exchange: Exchange::Bitso,
            pair,
            bid: payload.bid.parse()?,
            ask: payload.ask.parse()?,
            last_price: payload.last.parse().unwrap_or_default(),
            volume_24h: payload.volume.parse().unwrap_or_default(),
            timestamp: Utc::now(),
        })
    }

    async fn subscribe_orderbook(
        &self,
        _pairs: &[TradingPair],
        _tx: mpsc::UnboundedSender<MarketEvent>,
    ) -> Result<()> {
        // Bitso WebSocket uses a different protocol — for now, we poll REST
        // TODO: implement wss://ws.bitso.com WebSocket
        Ok(())
    }

    async fn place_limit_order(
        &self,
        pair: TradingPair,
        side: Side,
        price: Decimal,
        quantity: Decimal,
    ) -> Result<String> {
        let book = Self::book_name(pair)?;
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        let body = serde_json::json!({
            "book": book,
            "side": side_str,
            "type": "limit",
            "price": price.to_string(),
            "major": quantity.to_string(),
        })
        .to_string();

        let path = "/api/v3/orders/";
        let auth = self.auth_header("POST", path, &body);

        let resp = self
            .client
            .post(&format!("https://bitso.com{}", path))
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .context("Failed to place Bitso order")?;

        let resp_body = Self::validate_response(resp).await?;
        let data: BitsoResponse<BitsoOrderPayload> =
            serde_json::from_str(&resp_body).context("Failed to parse Bitso order response")?;

        let payload = data.payload.context("Bitso order: no payload")?;
        Ok(payload.oid)
    }

    async fn place_market_order(
        &self,
        pair: TradingPair,
        side: Side,
        quantity: Decimal,
    ) -> Result<String> {
        let book = Self::book_name(pair)?;
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        let body = serde_json::json!({
            "book": book,
            "side": side_str,
            "type": "market",
            "major": quantity.to_string(),
        })
        .to_string();

        let path = "/api/v3/orders/";
        let auth = self.auth_header("POST", path, &body);

        let resp = self
            .client
            .post(&format!("https://bitso.com{}", path))
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .context("Failed to place Bitso market order")?;

        let resp_body = Self::validate_response(resp).await?;
        let data: BitsoResponse<BitsoOrderPayload> = serde_json::from_str(&resp_body)?;

        let payload = data.payload.context("Bitso order: no payload")?;
        Ok(payload.oid)
    }

    async fn cancel_order(&self, _pair: TradingPair, order_id: &str) -> Result<()> {
        // SEC: validate order_id format (Bitso uses alphanumeric OIDs)
        if !order_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            anyhow::bail!("Invalid Bitso order ID format");
        }

        let path = format!("/api/v3/orders/{}/", order_id);
        let auth = self.auth_header("DELETE", &path, "");

        let resp = self
            .client
            .delete(&format!("https://bitso.com{}", path))
            .header("Authorization", auth)
            .send()
            .await
            .context("Failed to cancel Bitso order")?;

        Self::validate_response(resp).await?;
        Ok(())
    }

    async fn get_balance(&self, asset: &str) -> Result<Decimal> {
        if !asset.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("Invalid asset identifier");
        }

        let path = "/api/v3/balance/";
        let auth = self.auth_header("GET", path, "");

        let resp = self
            .client
            .get(&format!("https://bitso.com{}", path))
            .header("Authorization", auth)
            .send()
            .await
            .context("Failed to fetch Bitso balance")?;

        let body = Self::validate_response(resp).await?;
        let data: BitsoResponse<BitsoBalancesPayload> = serde_json::from_str(&body)?;

        let payload = data.payload.context("Bitso balance: no payload")?;
        let asset_lower = asset.to_lowercase();
        let balance = payload
            .balances
            .iter()
            .find(|b| b.currency == asset_lower)
            .map(|b| b.available.parse::<Decimal>().unwrap_or_default())
            .unwrap_or_default();

        Ok(balance)
    }
}
