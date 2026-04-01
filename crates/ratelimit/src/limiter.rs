use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::{Semaphore, mpsc};
use tokio::time::{Duration, sleep};
use tracing::warn;

use cripton_core::{Exchange, OrderBook, Side, Ticker, TradingPair};
use cripton_exchanges::traits::{ExchangeConnector, MarketEvent};

/// A rate-limited wrapper around any ExchangeConnector.
///
/// Uses a token bucket algorithm:
/// - `max_requests` tokens available per `window` duration
/// - Each API call consumes one token
/// - If no tokens available, the call waits until one is replenished
///
/// Binance limits: 1200 requests/minute (weight-based, simplified here)
/// Bitso limits: 300 requests/minute
pub struct RateLimitedExchange {
    inner: Arc<dyn ExchangeConnector>,
    semaphore: Arc<Semaphore>,
    refill_rate: Duration,
}

impl RateLimitedExchange {
    /// Create a rate-limited exchange.
    /// `max_requests_per_minute`: maximum API calls per minute.
    pub fn new(inner: Arc<dyn ExchangeConnector>, max_requests_per_minute: u32) -> Self {
        // Use 80% of the limit as safety margin
        let safe_limit = (max_requests_per_minute * 80 / 100).max(1) as usize;
        let refill_rate = Duration::from_millis(60_000 / safe_limit as u64);

        let semaphore = Arc::new(Semaphore::new(safe_limit));

        // Spawn token replenishment task
        let sem = semaphore.clone();
        let max = safe_limit;
        tokio::spawn(async move {
            loop {
                sleep(refill_rate).await;
                if sem.available_permits() < max {
                    sem.add_permits(1);
                }
            }
        });

        Self {
            inner,
            semaphore,
            refill_rate,
        }
    }

    /// Binance: 1200 req/min (uses 960 = 80%)
    pub fn binance(inner: Arc<dyn ExchangeConnector>) -> Self {
        Self::new(inner, 1200)
    }

    /// Bitso: 300 req/min (uses 240 = 80%)
    pub fn bitso(inner: Arc<dyn ExchangeConnector>) -> Self {
        Self::new(inner, 300)
    }

    async fn acquire(&self) {
        // Try to acquire a permit, wait if none available
        if self.semaphore.available_permits() == 0 {
            warn!(
                "Rate limit reached for {}, waiting {:.0}ms",
                self.inner.exchange(),
                self.refill_rate.as_millis()
            );
        }
        // acquire_owned would move the semaphore, so we use a simpler approach:
        // wait for a permit, then immediately release it (just as a gate)
        let permit = self.semaphore.acquire().await;
        if let Ok(p) = permit {
            p.forget(); // consume the token
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
