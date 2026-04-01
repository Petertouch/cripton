use chrono::{Datelike, NaiveTime, Utc, Weekday};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::{debug, info};

/// A time window with associated aggressiveness multiplier
#[derive(Debug, Clone)]
pub struct TradingWindow {
    pub name: &'static str,
    pub start: NaiveTime,
    pub end: NaiveTime,
    /// Multiplier for trade_amount during this window (1.0 = normal, 2.0 = double)
    pub aggression: Decimal,
    /// Minimum profit threshold override (lower = more sensitive to opportunities)
    pub min_profit_pct: Option<Decimal>,
    /// Which days this window applies (None = every day)
    pub days: Option<Vec<Weekday>>,
}

impl TradingWindow {
    fn is_active_now(&self) -> bool {
        let now = Utc::now();
        let time = now.time();
        let weekday = now.weekday();

        // Check day filter
        if let Some(ref days) = self.days {
            if !days.contains(&weekday) {
                return false;
            }
        }

        // Handle windows that cross midnight (e.g. 23:00 → 01:00)
        if self.start <= self.end {
            time >= self.start && time < self.end
        } else {
            time >= self.start || time < self.end
        }
    }
}

/// Scheduler configuration for a trading session
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Base trade amount when no window is active
    pub base_trade_amount: Decimal,
    /// Base minimum profit percentage
    pub base_min_profit_pct: Decimal,
    /// Whether trading is allowed outside defined windows
    pub allow_off_window: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            base_trade_amount: dec!(100),
            base_min_profit_pct: dec!(0.03),
            allow_off_window: true,
        }
    }
}

/// The trading schedule parameters returned for the current moment
#[derive(Debug, Clone)]
pub struct ActiveParams {
    pub trade_amount: Decimal,
    pub min_profit_pct: Decimal,
    pub active_window: Option<String>,
    pub is_aggressive: bool,
}

/// Manages trading windows and returns current operating parameters
pub struct Scheduler {
    config: SchedulerConfig,
    windows: Vec<TradingWindow>,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        let windows = Self::default_windows();
        info!(
            "Scheduler initialized with {} trading windows",
            windows.len()
        );
        Self { config, windows }
    }

    /// Get the current trading parameters based on active windows
    pub fn current_params(&self) -> ActiveParams {
        let active = self.active_window();

        match active {
            Some(window) => {
                let trade_amount = self.config.base_trade_amount * window.aggression;
                let min_profit = window
                    .min_profit_pct
                    .unwrap_or(self.config.base_min_profit_pct);

                debug!(
                    "Active window: {} | aggression: {}x | amount: {} | min_profit: {}%",
                    window.name, window.aggression, trade_amount, min_profit
                );

                ActiveParams {
                    trade_amount,
                    min_profit_pct: min_profit,
                    active_window: Some(window.name.to_string()),
                    is_aggressive: window.aggression > Decimal::ONE,
                }
            }
            None => {
                if !self.config.allow_off_window {
                    // Return zero amount to effectively pause trading
                    return ActiveParams {
                        trade_amount: Decimal::ZERO,
                        min_profit_pct: self.config.base_min_profit_pct,
                        active_window: None,
                        is_aggressive: false,
                    };
                }

                ActiveParams {
                    trade_amount: self.config.base_trade_amount,
                    min_profit_pct: self.config.base_min_profit_pct,
                    active_window: None,
                    is_aggressive: false,
                }
            }
        }
    }

    /// Check if trading is currently active
    pub fn is_trading_active(&self) -> bool {
        if self.config.allow_off_window {
            return true;
        }
        self.active_window().is_some()
    }

    fn active_window(&self) -> Option<&TradingWindow> {
        self.windows.iter().find(|w| w.is_active_now())
    }

    /// Default windows based on known high-volatility periods for stablecoins
    fn default_windows() -> Vec<TradingWindow> {
        vec![
            // Funding rate reset — stablecoin pairs adjust
            TradingWindow {
                name: "funding_reset",
                start: NaiveTime::from_hms_opt(23, 45, 0).unwrap_or_default(),
                end: NaiveTime::from_hms_opt(0, 30, 0).unwrap_or_default(),
                aggression: dec!(2.0),
                min_profit_pct: Some(dec!(0.02)),
                days: None,
            },
            // Asian market open — high stablecoin volume
            TradingWindow {
                name: "asia_open",
                start: NaiveTime::from_hms_opt(8, 0, 0).unwrap_or_default(),
                end: NaiveTime::from_hms_opt(9, 0, 0).unwrap_or_default(),
                aggression: dec!(1.5),
                min_profit_pct: Some(dec!(0.025)),
                days: None,
            },
            // US market open — maximum volatility
            TradingWindow {
                name: "us_open",
                start: NaiveTime::from_hms_opt(13, 0, 0).unwrap_or_default(),
                end: NaiveTime::from_hms_opt(14, 30, 0).unwrap_or_default(),
                aggression: dec!(2.5),
                min_profit_pct: Some(dec!(0.015)),
                days: None,
            },
            // European open
            TradingWindow {
                name: "europe_open",
                start: NaiveTime::from_hms_opt(7, 0, 0).unwrap_or_default(),
                end: NaiveTime::from_hms_opt(8, 0, 0).unwrap_or_default(),
                aggression: dec!(1.5),
                min_profit_pct: Some(dec!(0.025)),
                days: None,
            },
            // Forex open — moves COP and EUR heavily
            TradingWindow {
                name: "forex_open",
                start: NaiveTime::from_hms_opt(21, 0, 0).unwrap_or_default(),
                end: NaiveTime::from_hms_opt(22, 0, 0).unwrap_or_default(),
                aggression: dec!(2.0),
                min_profit_pct: Some(dec!(0.02)),
                days: Some(vec![Weekday::Sun]),
            },
            // Colombia market hours — best for COP arbitrage
            TradingWindow {
                name: "colombia_market",
                start: NaiveTime::from_hms_opt(14, 0, 0).unwrap_or_default(), // 9am COT = 14 UTC
                end: NaiveTime::from_hms_opt(21, 0, 0).unwrap_or_default(),   // 4pm COT = 21 UTC
                aggression: dec!(1.8),
                min_profit_pct: Some(dec!(0.02)),
                days: Some(vec![
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Fri,
                ]),
            },
        ]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_default_params_when_no_window() {
        let config = SchedulerConfig {
            allow_off_window: true,
            ..Default::default()
        };
        let scheduler = Scheduler::new(config);
        let params = scheduler.current_params();
        // Should always return valid params
        assert!(params.trade_amount > Decimal::ZERO);
    }

    #[test]
    fn test_paused_when_off_window_disabled() {
        let config = SchedulerConfig {
            allow_off_window: false,
            ..Default::default()
        };
        let scheduler = Scheduler {
            config,
            windows: vec![], // no windows defined
        };
        let params = scheduler.current_params();
        assert_eq!(params.trade_amount, Decimal::ZERO);
    }

    #[test]
    fn test_window_crossing_midnight() {
        let window = TradingWindow {
            name: "test",
            start: NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(1, 0, 0).unwrap(),
            aggression: dec!(2.0),
            min_profit_pct: None,
            days: None,
        };
        // This test validates the logic compiles and runs
        let _ = window.is_active_now();
    }
}
