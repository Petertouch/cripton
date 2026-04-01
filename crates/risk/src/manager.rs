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
            max_trade_amount: dec!(500),       // max $500 per trade
            max_total_exposure: dec!(2000),     // max $2000 total
            max_loss: dec!(50),                 // trip at $50 loss
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
    pub fn validate(&mut self, signals: &[Signal]) -> Vec<Signal> {
        // Check circuit breaker first
        if !self.circuit_breaker.is_trading_allowed() {
            warn!("Risk: circuit breaker is active, rejecting all signals");
            return vec![];
        }

        let mut approved = Vec::new();

        for signal in signals {
            let trade_value = signal.quantity * signal.price.unwrap_or(Decimal::ONE);

            // Check max trade amount
            if trade_value > self.config.max_trade_amount {
                warn!(
                    "Risk: trade value {:.2} exceeds max {:.2} for {} {}",
                    trade_value, self.config.max_trade_amount, signal.pair, signal.side
                );
                continue;
            }

            // Check total exposure
            if self.current_exposure + trade_value > self.config.max_total_exposure {
                warn!(
                    "Risk: total exposure would be {:.2}, exceeds max {:.2}",
                    self.current_exposure + trade_value,
                    self.config.max_total_exposure
                );
                continue;
            }

            approved.push(signal.clone());
        }

        if approved.len() != signals.len() {
            info!(
                "Risk: approved {}/{} signals",
                approved.len(),
                signals.len()
            );
        }

        approved
    }

    /// Record the P&L of a completed trade
    pub fn record_trade_pnl(&mut self, pnl: Decimal) {
        self.circuit_breaker.record_pnl(pnl);
    }

    /// Update current exposure (call after fills)
    pub fn update_exposure(&mut self, exposure: Decimal) {
        self.current_exposure = exposure;
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
}
