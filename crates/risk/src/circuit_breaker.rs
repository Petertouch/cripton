use std::collections::VecDeque;

use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use tracing::{error, warn};

/// Circuit breaker that halts trading when losses exceed thresholds.
///
/// Tracks recent P&L and trips if:
/// - Total loss in the window exceeds max_loss
/// - Number of consecutive losses exceeds max_consecutive_losses
#[derive(Debug)]
pub struct CircuitBreaker {
    /// Maximum allowed loss in the time window
    max_loss: Decimal,
    /// Time window to measure losses
    window: Duration,
    /// Max consecutive losing trades before tripping
    max_consecutive_losses: u32,
    /// Cooldown period after tripping
    cooldown: Duration,

    /// Recent P&L entries: (timestamp, pnl)
    recent_pnl: VecDeque<(DateTime<Utc>, Decimal)>,
    /// Current consecutive loss count
    consecutive_losses: u32,
    /// When the breaker was tripped (None = not tripped)
    tripped_at: Option<DateTime<Utc>>,
}

impl CircuitBreaker {
    pub fn new(
        max_loss: Decimal,
        window_minutes: i64,
        max_consecutive_losses: u32,
        cooldown_minutes: i64,
    ) -> Self {
        Self {
            max_loss,
            window: Duration::minutes(window_minutes),
            max_consecutive_losses,
            cooldown: Duration::minutes(cooldown_minutes),
            recent_pnl: VecDeque::new(),
            consecutive_losses: 0,
            tripped_at: None,
        }
    }

    /// Record a trade's P&L result
    pub fn record_pnl(&mut self, pnl: Decimal) {
        let now = Utc::now();
        self.recent_pnl.push_back((now, pnl));
        self.prune_old_entries(now);

        if pnl < Decimal::ZERO {
            // SEC: use saturating_add to prevent integer overflow
            self.consecutive_losses = self.consecutive_losses.saturating_add(1);
        } else {
            self.consecutive_losses = 0;
        }

        // Check if we should trip
        if self.should_trip() {
            self.trip();
        }
    }

    /// Check if trading is currently allowed
    pub fn is_trading_allowed(&mut self) -> bool {
        // If not tripped, allow
        let Some(tripped_at) = self.tripped_at else {
            return true;
        };

        // SEC: check cooldown with monotonic-safe comparison
        // If clock goes backward, now < tripped_at, we stay tripped (safe default)
        let now = Utc::now();
        if now > tripped_at {
            if now - tripped_at >= self.cooldown {
                warn!("Circuit breaker cooldown expired, resuming trading");
                self.reset();
                return true;
            }
        }
        // If now <= tripped_at (clock skew), remain tripped (fail-safe)

        false
    }

    /// Get the total P&L in the current window
    pub fn window_pnl(&self) -> Decimal {
        self.recent_pnl.iter().map(|(_, pnl)| pnl).sum()
    }

    /// Check if the breaker is currently tripped
    pub fn is_tripped(&self) -> bool {
        self.tripped_at.is_some()
    }

    fn should_trip(&self) -> bool {
        // Trip on total loss threshold
        let total_loss = self.window_pnl();
        if total_loss < -self.max_loss {
            error!(
                "CIRCUIT BREAKER: Total loss {:.4} exceeds max {:.4}",
                total_loss, self.max_loss
            );
            return true;
        }

        // Trip on consecutive losses
        if self.consecutive_losses >= self.max_consecutive_losses {
            error!(
                "CIRCUIT BREAKER: {} consecutive losses (max {})",
                self.consecutive_losses, self.max_consecutive_losses
            );
            return true;
        }

        false
    }

    fn trip(&mut self) {
        error!("CIRCUIT BREAKER TRIPPED — halting all trading for {} minutes", self.cooldown.num_minutes());
        self.tripped_at = Some(Utc::now());
    }

    fn reset(&mut self) {
        self.tripped_at = None;
        self.consecutive_losses = 0;
        self.recent_pnl.clear();
    }

    fn prune_old_entries(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.window;
        while let Some((ts, _)) = self.recent_pnl.front() {
            if *ts < cutoff {
                self.recent_pnl.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_allows_trading_initially() {
        let mut cb = CircuitBreaker::new(dec!(10), 60, 5, 30);
        assert!(cb.is_trading_allowed());
    }

    #[test]
    fn test_trips_on_max_loss() {
        let mut cb = CircuitBreaker::new(dec!(10), 60, 5, 30);
        cb.record_pnl(dec!(-5));
        assert!(cb.is_trading_allowed());
        cb.record_pnl(dec!(-6)); // total = -11, exceeds -10
        assert!(!cb.is_trading_allowed());
    }

    #[test]
    fn test_trips_on_consecutive_losses() {
        let mut cb = CircuitBreaker::new(dec!(100), 60, 3, 30);
        cb.record_pnl(dec!(-0.01));
        cb.record_pnl(dec!(-0.01));
        assert!(cb.is_trading_allowed());
        cb.record_pnl(dec!(-0.01)); // 3rd consecutive
        assert!(!cb.is_trading_allowed());
    }

    #[test]
    fn test_profit_resets_consecutive() {
        let mut cb = CircuitBreaker::new(dec!(100), 60, 3, 30);
        cb.record_pnl(dec!(-0.01));
        cb.record_pnl(dec!(-0.01));
        cb.record_pnl(dec!(0.05)); // profit resets
        cb.record_pnl(dec!(-0.01));
        assert!(cb.is_trading_allowed()); // only 1 consecutive loss
    }
}
