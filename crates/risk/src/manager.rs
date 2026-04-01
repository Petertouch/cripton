use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::{info, warn};

use cripton_core::Signal;

use crate::circuit_breaker::CircuitBreaker;

/// Risk management configuration
#[derive(Debug, Clone)]
pub struct RiskConfig {
    /// Maximum amount per single trade
    pub max_trade_amount: Decimal,
    /// Maximum total exposure across all open positions
    pub max_total_exposure: Decimal,
    /// Maximum loss before circuit breaker trips (in quote currency)
    pub max_loss: Decimal,
    /// Circuit breaker window in minutes
    pub cb_window_minutes: i64,
    /// Max consecutive losses before circuit breaker trips
    pub max_consecutive_losses: u32,
    /// Cooldown after circuit breaker trips, in minutes
    pub cb_cooldown_minutes: i64,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_trade_amount: dec!(500),
            max_total_exposure: dec!(2000),
            max_loss: dec!(50),
            cb_window_minutes: 60,
            max_consecutive_losses: 5,
            cb_cooldown_minutes: 30,
        }
    }
}

/// Central risk manager that validates signals before execution
pub struct RiskManager {
    config: RiskConfig,
    circuit_breaker: CircuitBreaker,
    current_exposure: Decimal,
}

impl RiskManager {
    pub fn new(config: RiskConfig) -> Self {
        let circuit_breaker = CircuitBreaker::new(
            config.max_loss,
            config.cb_window_minutes,
            config.max_consecutive_losses,
            config.cb_cooldown_minutes,
        );

        Self {
            config,
            circuit_breaker,
            current_exposure: Decimal::ZERO,
        }
    }

    /// Validate a set of signals against risk rules.
    /// Returns the filtered signals that pass all checks.
    /// SEC: increments pending_exposure for each approved signal to prevent
    /// bypass via concurrent validate() calls.
    pub fn validate(&mut self, signals: &[Signal]) -> Vec<Signal> {
        if !self.circuit_breaker.is_trading_allowed() {
            warn!("Risk: circuit breaker is active, rejecting all signals");
            return vec![];
        }

        let mut approved = Vec::new();
        // SEC: track cumulative exposure across signals in this batch
        let mut batch_exposure = Decimal::ZERO;

        for signal in signals {
            let trade_value = signal.quantity * signal.price.unwrap_or(Decimal::ONE);

            if trade_value > self.config.max_trade_amount {
                warn!(
                    "Risk: trade value exceeds per-trade limit for {} {}",
                    signal.pair, signal.side
                );
                continue;
            }

            let projected_exposure = self.current_exposure + batch_exposure + trade_value;
            if projected_exposure > self.config.max_total_exposure {
                warn!(
                    "Risk: projected exposure would exceed limit for {} {}",
                    signal.pair, signal.side
                );
                continue;
            }

            // SEC: increment batch exposure so next signal in batch sees accurate total
            batch_exposure += trade_value;
            approved.push(signal.clone());
        }

        // SEC: commit the batch exposure as pending
        self.current_exposure += batch_exposure;

        if approved.len() != signals.len() {
            info!(
                "Risk: approved {}/{} signals (exposure: {:.2})",
                approved.len(),
                signals.len(),
                self.current_exposure
            );
        }

        approved
    }

    /// Record the P&L of a completed trade and release exposure
    pub fn record_trade_result(&mut self, pnl: Decimal, released_exposure: Decimal) {
        self.circuit_breaker.record_pnl(pnl);
        // SEC: release exposure from completed trades
        self.current_exposure = (self.current_exposure - released_exposure).max(Decimal::ZERO);
    }

    /// Check if trading is allowed
    pub fn is_trading_allowed(&mut self) -> bool {
        self.circuit_breaker.is_trading_allowed()
    }

    /// Get circuit breaker status for monitoring
    pub fn circuit_breaker_status(&self) -> (bool, Decimal) {
        (
            self.circuit_breaker.is_tripped(),
            self.circuit_breaker.window_pnl(),
        )
    }

    /// Current exposure for monitoring
    pub fn current_exposure(&self) -> Decimal {
        self.current_exposure
    }
}
