use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, Instant, sleep};
use tracing::warn;

use cripton_core::{Exchange, OrderBook, Side, Ticker, TradingPair};
use cripton_exchanges::traits::{ExchangeConnector, MarketEvent};

/// Token bucket state — tracks available tokens and refill timing
struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64, // tokens per millisecond
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_requests_per_minute: u32) -> Self {
        // Use 80% of the limit as safety margin
        let safe_limit = (max_requests_per_minute as f64) * 0.8;
        Self {
            tokens: safe_limit,
            max_tokens: safe_limit,
            refill_rate: safe_limit / 60_000.0, // tokens per ms
            last_refill: Instant::now(),
        }
    }

    /// Try to consume one token. Returns wait duration if no tokens available.
    fn try_acquire(&mut self) -> Option<Duration> {
        // Refill tokens based on elapsed time
        let now = Instant::now();
        let elapsed_ms = now.duration_since(self.last_refill).as_millis() as f64;
        self.tokens = (self.tokens + elapsed_ms * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            None // acquired successfully
        } else {
            // Calculate how long to wait for 1 token
            let deficit = 1.0 - self.tokens;
            let wait_ms = (deficit / self.refill_rate).ceil() as u64;
            Some(Duration::from_millis(wait_ms.max(1)))
        }
    }
}

/// A rate-limited wrapper around any ExchangeConnector.
///
/// Uses a proper token bucket algorithm:
/// - Tokens refill continuously based on elapsed time
/// - Each API call consumes one token
/// - If no tokens available, the call waits the exact time needed
/// - No leaked permits, no semaphore, no background tasks
pub struct RateLimitedExchange {
    inner: Arc<dyn ExchangeConnector>,
    bucket: Arc<Mutex<TokenBucket>>,
}

impl RateLimitedExchange {
    pub fn new(inner: Arc<dyn ExchangeConnector>, max_requests_per_minute: u32) -> Self {
        Self {
            inner,
            bucket: Arc::new(Mutex::new(TokenBucket::new(max_requests_per_minute))),
        }
    }

    /// Binance: 1200 req/min
    pub fn binance(inner: Arc<dyn ExchangeConnector>) -> Self {
        Self::new(inner, 1200)
    }

    /// Bitso: 300 req/min
    pub fn bitso(inner: Arc<dyn ExchangeConnector>) -> Self {
        Self::new(inner, 300)
    }

    async fn acquire(&self) {
        loop {
            let wait = {
                let mut bucket = self.bucket.lock().await;
                bucket.try_acquire()
            };

            match wait {
                None => return, // token acquired
                Some(duration) => {
                    warn!(
                        "Rate limit hit for {} — waiting {}ms",
                        self.inner.exchange(),
                        duration.as_millis()
                    );
                    sleep(duration).await;
                }
            }
        }
    }
}

#[async_trait]
impl ExchangeConnector for RateLimitedExchange {
    fn exchange(&self) -> Exchange {
        self.inner.exchange()
    }

    async fn fetch_orderbook(&self, pair: TradingPair) -> Result<OrderBook> {
        self.acquire().await;
        self.inner.fetch_orderbook(pair).await
    }

    async fn fetch_ticker(&self, pair: TradingPair) -> Result<Ticker> {
        self.acquire().await;
        self.inner.fetch_ticker(pair).await
    }

    async fn subscribe_orderbook(
        &self,
        pairs: &[TradingPair],
        tx: mpsc::UnboundedSender<MarketEvent>,
    ) -> Result<()> {
        // WebSocket subscription doesn't count as REST request
        self.inner.subscribe_orderbook(pairs, tx).await
    }

    async fn place_limit_order(
        &self,
        pair: TradingPair,
        side: Side,
        price: Decimal,
        quantity: Decimal,
    ) -> Result<String> {
        self.acquire().await;
        self.inner
            .place_limit_order(pair, side, price, quantity)
            .await
    }

    async fn place_market_order(
        &self,
        pair: TradingPair,
        side: Side,
        quantity: Decimal,
    ) -> Result<String> {
        self.acquire().await;
        self.inner.place_market_order(pair, side, quantity).await
    }

    async fn cancel_order(&self, pair: TradingPair, order_id: &str) -> Result<()> {
        self.acquire().await;
        self.inner.cancel_order(pair, order_id).await
    }

    async fn get_balance(&self, asset: &str) -> Result<Decimal> {
        self.acquire().await;
        self.inner.get_balance(asset).await
    }
}
