use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::{RwLock, mpsc};
use tracing::info;

use cripton_core::{Exchange, OrderBook, Side, Ticker, TradingPair};
use cripton_exchanges::traits::{ExchangeConnector, MarketEvent};

/// A paper trading exchange that simulates order fills using real market data.
///
/// - Wraps a real exchange connector for market data (orderbook, ticker, WS)
/// - Simulates order execution at current market prices with configurable slippage
/// - Tracks virtual balances
/// - Logs every simulated trade for analysis
pub struct PaperExchange {
    /// The real exchange we read market data from
    real_exchange: Arc<dyn ExchangeConnector>,
    /// Virtual balances per asset
    balances: Arc<RwLock<HashMap<String, Decimal>>>,
    /// Simulated fill slippage (e.g. 0.0005 = 0.05%)
    slippage_rate: Decimal,
    /// Counter for generating unique order IDs
    order_counter: AtomicU64,
    /// Total simulated P&L
    total_pnl: Arc<RwLock<Decimal>>,
    /// Trade log: (timestamp, pair, side, price, qty, fee)
    trade_log: Arc<RwLock<Vec<PaperTrade>>>,
}

#[derive(Debug, Clone)]
pub struct PaperTrade {
    pub id: String,
    pub pair: TradingPair,
    pub side: Side,
    pub price: Decimal,
    pub quantity: Decimal,
    pub fee: Decimal,
    pub timestamp: chrono::DateTime<Utc>,
}

impl PaperExchange {
    pub fn new(
        real_exchange: Arc<dyn ExchangeConnector>,
        initial_balances: HashMap<String, Decimal>,
        slippage_rate: Decimal,
    ) -> Self {
        info!(
            "Paper trading initialized for {} with {} assets",
            real_exchange.exchange(),
            initial_balances.len()
        );
        for (asset, amount) in &initial_balances {
            info!("  {}: {}", asset, amount);
        }

        // SEC: clamp slippage to [0, 0.05] (0-5%) — reject negative or extreme values
        let clamped_slippage = slippage_rate.max(Decimal::ZERO).min(dec!(0.05));
        if clamped_slippage != slippage_rate {
            info!(
                "Paper: slippage clamped from {} to {}",
                slippage_rate, clamped_slippage
            );
        }

        Self {
            real_exchange,
            balances: Arc::new(RwLock::new(initial_balances)),
            slippage_rate: clamped_slippage,
            order_counter: AtomicU64::new(1),
            total_pnl: Arc::new(RwLock::new(Decimal::ZERO)),
            trade_log: Arc::new(RwLock::new(Vec::new())),
        }
    }

    fn next_order_id(&self) -> String {
        let id = self.order_counter.fetch_add(1, Ordering::SeqCst);
        format!("PAPER-{}", id)
    }

    /// Apply simulated slippage to a price (always works against the trader)
    fn apply_slippage(&self, price: Decimal, side: Side) -> Decimal {
        match side {
            Side::Buy => price * (Decimal::ONE + self.slippage_rate),
            Side::Sell => price * (Decimal::ONE - self.slippage_rate),
        }
    }

    /// Simulate filling an order at current market price
    async fn simulate_fill(
        &self,
        pair: TradingPair,
        side: Side,
        quantity: Decimal,
        limit_price: Option<Decimal>,
    ) -> Result<String> {
        // Fetch real market data
        let book = self.real_exchange.fetch_orderbook(pair).await?;

        let market_price = match side {
            Side::Buy => book.best_ask().map(|l| l.price).unwrap_or(Decimal::ZERO),
            Side::Sell => book.best_bid().map(|l| l.price).unwrap_or(Decimal::ZERO),
        };

        if market_price.is_zero() {
            anyhow::bail!("Paper: no market data for {}", pair);
        }

        // Apply simulated slippage
        let fill_price = self.apply_slippage(market_price, side);

        // Check limit price if provided
        if let Some(limit) = limit_price {
            match side {
                Side::Buy if fill_price > limit => {
                    anyhow::bail!(
                        "Paper: fill price {} exceeds limit {} for BUY {}",
                        fill_price,
                        limit,
                        pair
                    );
                }
                Side::Sell if fill_price < limit => {
                    anyhow::bail!(
                        "Paper: fill price {} below limit {} for SELL {}",
                        fill_price,
                        limit,
                        pair
                    );
                }
                _ => {}
            }
        }

        // Calculate fee (0.1% default)
        let notional = quantity * fill_price;
        let fee = notional * dec!(0.001);

        // Update virtual balances
        let base = pair.base().to_string();
        let quote = pair.quote().to_string();
        {
            let mut bals = self.balances.write().await;
            match side {
                Side::Buy => {
                    // Spend quote, receive base
                    let quote_bal = bals.entry(quote.clone()).or_insert(Decimal::ZERO);
                    if *quote_bal < notional + fee {
                        anyhow::bail!(
                            "Paper: insufficient {} balance ({} < {})",
                            quote,
                            quote_bal,
                            notional + fee
                        );
                    }
                    *quote_bal -= notional + fee;
                    *bals.entry(base.clone()).or_insert(Decimal::ZERO) += quantity;
                }
                Side::Sell => {
                    // Spend base, receive quote
                    let base_bal = bals.entry(base.clone()).or_insert(Decimal::ZERO);
                    if *base_bal < quantity {
                        anyhow::bail!(
                            "Paper: insufficient {} balance ({} < {})",
                            base,
                            base_bal,
                            quantity
                        );
                    }
                    *base_bal -= quantity;
                    *bals.entry(quote.clone()).or_insert(Decimal::ZERO) += notional - fee;
                }
            }
        }

        // Track P&L (fee is always a cost)
        {
            let mut pnl = self.total_pnl.write().await;
            *pnl -= fee;
        }

        // Log the trade
        let order_id = self.next_order_id();
        {
            let mut log = self.trade_log.write().await;
            log.push(PaperTrade {
                id: order_id.clone(),
                pair,
                side,
                price: fill_price,
                quantity,
                fee,
                timestamp: Utc::now(),
            });
        }

        info!(
            "PAPER {} {} {} @ {} (market: {}) | fee: {} | id: {}",
            side, quantity, pair, fill_price, market_price, fee, order_id
        );

        Ok(order_id)
    }

    /// Get current virtual balances
    pub async fn balances(&self) -> HashMap<String, Decimal> {
        self.balances.read().await.clone()
    }

    /// Get total simulated P&L
    pub async fn pnl(&self) -> Decimal {
        *self.total_pnl.read().await
    }

    /// Get all simulated trades
    pub async fn trades(&self) -> Vec<PaperTrade> {
        self.trade_log.read().await.clone()
    }

    /// Print a summary of paper trading results
    pub async fn print_summary(&self) {
        let bals = self.balances.read().await;
        let pnl = self.total_pnl.read().await;
        let trades = self.trade_log.read().await;

        info!("=== PAPER TRADING SUMMARY ===");
        info!("  Total trades: {}", trades.len());
        info!("  Total P&L (fees): {:.6}", *pnl);
        info!("  Balances:");
        for (asset, amount) in bals.iter() {
            if *amount != Decimal::ZERO {
                info!("    {}: {:.6}", asset, amount);
            }
        }
    }
}

#[async_trait]
impl ExchangeConnector for PaperExchange {
    fn exchange(&self) -> Exchange {
        self.real_exchange.exchange()
    }

    async fn fetch_orderbook(&self, pair: TradingPair) -> Result<OrderBook> {
        self.real_exchange.fetch_orderbook(pair).await
    }

    async fn fetch_ticker(&self, pair: TradingPair) -> Result<Ticker> {
        self.real_exchange.fetch_ticker(pair).await
    }

    async fn subscribe_orderbook(
        &self,
        pairs: &[TradingPair],
        tx: mpsc::UnboundedSender<MarketEvent>,
    ) -> Result<()> {
        self.real_exchange.subscribe_orderbook(pairs, tx).await
    }

    async fn place_limit_order(
        &self,
        pair: TradingPair,
        side: Side,
        price: Decimal,
        quantity: Decimal,
    ) -> Result<String> {
        self.simulate_fill(pair, side, quantity, Some(price)).await
    }

    async fn place_market_order(
        &self,
        pair: TradingPair,
        side: Side,
        quantity: Decimal,
    ) -> Result<String> {
        self.simulate_fill(pair, side, quantity, None).await
    }

    async fn cancel_order(&self, _pair: TradingPair, order_id: &str) -> Result<()> {
        info!("PAPER: cancel order {} (no-op in paper mode)", order_id);
        Ok(())
    }

    async fn get_balance(&self, asset: &str) -> Result<Decimal> {
        let bals = self.balances.read().await;
        Ok(*bals.get(asset).unwrap_or(&Decimal::ZERO))
    }
}
