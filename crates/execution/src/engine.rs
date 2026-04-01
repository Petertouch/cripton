use std::sync::Arc;

use anyhow::Result;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use cripton_core::{MarketState, OrderStatus, Signal, Trade};
use cripton_exchanges::ExchangeConnector;

use crate::order_manager::OrderManager;
use crate::slippage;

/// Configuration for the execution engine
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    /// Maximum slippage tolerance as percentage (e.g. 0.05 = 0.05%)
    pub max_slippage_pct: Decimal,
    /// Maximum number of retry attempts for failed orders
    pub max_retries: u32,
    /// Whether to use limit orders (true) or market orders (false)
    pub use_limit_orders: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_slippage_pct: dec!(0.05),
            max_retries: 2,
            use_limit_orders: true,
        }
    }
}

/// The execution engine receives signals and places orders on exchanges
pub struct ExecutionEngine {
    exchanges: Vec<Arc<dyn ExchangeConnector>>,
    order_manager: Arc<Mutex<OrderManager>>,
    config: ExecutionConfig,
}

impl ExecutionEngine {
    pub fn new(
        exchanges: Vec<Arc<dyn ExchangeConnector>>,
        config: ExecutionConfig,
    ) -> Self {
        Self {
            exchanges,
            order_manager: Arc::new(Mutex::new(OrderManager::new())),
            config,
        }
    }

    pub fn order_manager(&self) -> Arc<Mutex<OrderManager>> {
        self.order_manager.clone()
    }

    fn get_exchange(&self, exchange: cripton_core::Exchange) -> Option<&Arc<dyn ExchangeConnector>> {
        self.exchanges.iter().find(|e| e.exchange() == exchange)
    }

    /// Execute a batch of signals (e.g. 3 legs of a triangle).
    /// For triangular arb, all legs must succeed or we abort.
    pub async fn execute_signals(
        &self,
        signals: &[Signal],
        _state: &MarketState,
    ) -> Result<Vec<Trade>> {
        if signals.is_empty() {
            return Ok(vec![]);
        }

        info!("Executing {} signals", signals.len());

        let mut trades = Vec::new();

        for (i, signal) in signals.iter().enumerate() {
            // SEC: reject zero-quantity legs — don't silently skip
            if signal.quantity.is_zero() {
                if i > 0 {
                    warn!("Leg {} has zero quantity — aborting remaining legs to prevent imbalance", i + 1);
                } else {
                    warn!("Leg 1 has zero quantity — skipping entire batch");
                }
                break;
            }

            // SEC: reject negative quantities
            if signal.quantity < Decimal::ZERO {
                error!("Leg {} has negative quantity — aborting", i + 1);
                break;
            }

            info!(
                "  Leg {}: {} {} {} @ {:?} qty={}",
                i + 1, signal.side, signal.pair, signal.exchange,
                signal.price, signal.quantity
            );

            let exchange = match self.get_exchange(signal.exchange) {
                Some(e) => e,
                None => {
                    error!("Exchange {:?} not configured — aborting", signal.exchange);
                    break;
                }
            };

            // SEC: hold lock for entire create → submit → update cycle (atomic)
            let mut mgr = self.order_manager.lock().await;
            let order = mgr.create_order(signal);
            let local_id = order.local_id.clone();

            // Place order on exchange (lock still held to prevent TOCTOU)
            let result = if self.config.use_limit_orders {
                if let Some(price) = signal.price {
                    let limit_price = slippage::apply_slippage(
                        price,
                        signal.side,
                        self.config.max_slippage_pct,
                    );
                    exchange
                        .place_limit_order(signal.pair, signal.side, limit_price, signal.quantity)
                        .await
                } else {
                    exchange
                        .place_market_order(signal.pair, signal.side, signal.quantity)
                        .await
                }
            } else {
                exchange
                    .place_market_order(signal.pair, signal.side, signal.quantity)
                    .await
            };

            match result {
                Ok(exchange_order_id) => {
                    info!("  Order placed: exchange_id={}", exchange_order_id);
                    mgr.set_exchange_id(&local_id, &exchange_order_id);
                    mgr.update_status(&local_id, OrderStatus::Filled);
                    drop(mgr); // release lock before building trade

                    trades.push(Trade {
                        id: uuid::Uuid::new_v4().to_string(),
                        order_id: exchange_order_id,
                        exchange: signal.exchange,
                        pair: signal.pair,
                        side: signal.side,
                        price: signal.price.unwrap_or_default(),
                        quantity: signal.quantity,
                        fee: signal.quantity * signal.price.unwrap_or_default() * dec!(0.001),
                        fee_currency: signal.pair.quote().to_string(),
                        timestamp: chrono::Utc::now(),
                    });
                }
                Err(e) => {
                    error!("  Order FAILED: {}", e);
                    mgr.update_status(&local_id, OrderStatus::Rejected);
                    drop(mgr);

                    warn!("  Aborting remaining legs due to failure");
                    break;
                }
            }
        }

        info!(
            "Execution complete: {}/{} legs filled",
            trades.len(),
            signals.len()
        );

        Ok(trades)
    }
}
