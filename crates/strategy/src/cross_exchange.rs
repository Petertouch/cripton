use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::{debug, info};

use cripton_core::{Exchange, MarketState, OrderType, Side, Signal, TradingPair};

use crate::traits::Strategy;

/// Cross-exchange arbitrage strategy.
///
/// Detects price differences for the same pair across exchanges:
///   Buy USDT/COP cheap on Bitso → Sell USDT/COP expensive on Binance (or vice versa)
///
/// Requires pre-funded accounts on both exchanges.
pub struct CrossExchangeArbitrage {
    /// Minimum profit percentage after fees
    pub min_profit_pct: Decimal,
    /// Fee rate on exchange A (e.g. Binance)
    pub fee_rate_a: Decimal,
    /// Fee rate on exchange B (e.g. Bitso)
    pub fee_rate_b: Decimal,
    /// Amount to trade per opportunity
    pub trade_amount: Decimal,
    /// Pairs to monitor across exchanges
    pub pairs: Vec<CrossPairConfig>,
}

/// Configuration for a single cross-exchange pair
#[derive(Debug, Clone)]
pub struct CrossPairConfig {
    pub pair: TradingPair,
    pub exchange_a: Exchange,
    pub exchange_b: Exchange,
}

impl CrossExchangeArbitrage {
    pub fn new(
        min_profit_pct: Decimal,
        fee_rate_a: Decimal,
        fee_rate_b: Decimal,
        trade_amount: Decimal,
        pairs: Vec<CrossPairConfig>,
    ) -> Self {
        Self {
            min_profit_pct,
            fee_rate_a,
            fee_rate_b,
            trade_amount,
            pairs,
        }
    }

    fn evaluate_pair(&self, config: &CrossPairConfig, state: &MarketState) -> Option<Vec<Signal>> {
        let book_a = state.get_orderbook(config.exchange_a, config.pair)?;
        let book_b = state.get_orderbook(config.exchange_b, config.pair)?;

        let bid_a = book_a.best_bid()?.price; // sell price on A
        let ask_a = book_a.best_ask()?.price; // buy price on A
        let bid_b = book_b.best_bid()?.price; // sell price on B
        let ask_b = book_b.best_ask()?.price; // buy price on B

        if ask_a.is_zero() || ask_b.is_zero() || bid_a.is_zero() || bid_b.is_zero() {
            return None;
        }

        let fee_mult_a = Decimal::ONE - self.fee_rate_a;
        let fee_mult_b = Decimal::ONE - self.fee_rate_b;

        // Direction 1: Buy on A, Sell on B
        // Profit = (bid_B * fee_B) - (ask_A * (1/fee_A))
        // Simplified: buy at ask_A, sell at bid_B, subtract both fees
        let revenue_sell_b = bid_b.checked_mul(fee_mult_b)?;
        let cost_buy_a = ask_a.checked_div(fee_mult_a)?;
        let profit_a_to_b = revenue_sell_b.checked_sub(cost_buy_a)?;
        let profit_pct_a_to_b = profit_a_to_b
            .checked_div(cost_buy_a)?
            .checked_mul(dec!(100))?;

        // Direction 2: Buy on B, Sell on A
        let revenue_sell_a = bid_a.checked_mul(fee_mult_a)?;
        let cost_buy_b = ask_b.checked_div(fee_mult_b)?;
        let profit_b_to_a = revenue_sell_a.checked_sub(cost_buy_b)?;
        let profit_pct_b_to_a = profit_b_to_a
            .checked_div(cost_buy_b)?
            .checked_mul(dec!(100))?;

        debug!(
            "CrossExchange {} | {}->{}: {:.4}% | {}->{}: {:.4}%",
            config.pair,
            config.exchange_a,
            config.exchange_b,
            profit_pct_a_to_b,
            config.exchange_b,
            config.exchange_a,
            profit_pct_b_to_a
        );

        let now = Utc::now();

        // Check direction 1: Buy A → Sell B
        if profit_pct_a_to_b > self.min_profit_pct {
            info!(
                "CROSS-EXCHANGE: Buy {} on {} @ {}, Sell on {} @ {} | profit: {:.4}%",
                config.pair, config.exchange_a, ask_a, config.exchange_b, bid_b, profit_pct_a_to_b
            );

            return Some(vec![
                Signal {
                    exchange: config.exchange_a,
                    pair: config.pair,
                    side: Side::Buy,
                    order_type: OrderType::Limit,
                    price: Some(ask_a),
                    quantity: self.trade_amount,
                    reason: format!(
                        "cross_arb buy_{}→sell_{} profit={:.4}%",
                        config.exchange_a, config.exchange_b, profit_pct_a_to_b
                    ),
                    timestamp: now,
                },
                Signal {
                    exchange: config.exchange_b,
                    pair: config.pair,
                    side: Side::Sell,
                    order_type: OrderType::Limit,
                    price: Some(bid_b),
                    quantity: self.trade_amount,
                    reason: format!(
                        "cross_arb buy_{}→sell_{} profit={:.4}%",
                        config.exchange_a, config.exchange_b, profit_pct_a_to_b
                    ),
                    timestamp: now,
                },
            ]);
        }

        // Check direction 2: Buy B → Sell A
        if profit_pct_b_to_a > self.min_profit_pct {
            info!(
                "CROSS-EXCHANGE: Buy {} on {} @ {}, Sell on {} @ {} | profit: {:.4}%",
                config.pair, config.exchange_b, ask_b, config.exchange_a, bid_a, profit_pct_b_to_a
            );

            return Some(vec![
                Signal {
                    exchange: config.exchange_b,
                    pair: config.pair,
                    side: Side::Buy,
                    order_type: OrderType::Limit,
                    price: Some(ask_b),
                    quantity: self.trade_amount,
                    reason: format!(
                        "cross_arb buy_{}→sell_{} profit={:.4}%",
                        config.exchange_b, config.exchange_a, profit_pct_b_to_a
                    ),
                    timestamp: now,
                },
                Signal {
                    exchange: config.exchange_a,
                    pair: config.pair,
                    side: Side::Sell,
                    order_type: OrderType::Limit,
                    price: Some(bid_a),
                    quantity: self.trade_amount,
                    reason: format!(
                        "cross_arb buy_{}→sell_{} profit={:.4}%",
                        config.exchange_b, config.exchange_a, profit_pct_b_to_a
                    ),
                    timestamp: now,
                },
            ]);
        }

        None
    }
}

#[async_trait]
impl Strategy for CrossExchangeArbitrage {
    fn name(&self) -> &str {
        "CrossExchangeArbitrage"
    }

    async fn evaluate(&self, state: &MarketState) -> Vec<Signal> {
        let mut all_signals = Vec::new();

        for config in &self.pairs {
            if let Some(signals) = self.evaluate_pair(config, state) {
                all_signals.extend(signals);
            }
        }

        all_signals
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use cripton_core::{OrderBook, PriceLevel};

    fn make_book(exchange: Exchange, pair: TradingPair, bid: &str, ask: &str) -> OrderBook {
        OrderBook {
            exchange,
            pair,
            bids: vec![PriceLevel {
                price: bid.parse().unwrap(),
                quantity: dec!(10000),
            }],
            asks: vec![PriceLevel {
                price: ask.parse().unwrap(),
                quantity: dec!(10000),
            }],
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_detects_cross_exchange_opportunity() {
        let strategy = CrossExchangeArbitrage::new(
            dec!(0.1), // min 0.1% profit
            dec!(0.001),
            dec!(0.006), // Bitso higher fees
            dec!(100),
            vec![CrossPairConfig {
                pair: TradingPair::UsdtCop,
                exchange_a: Exchange::Binance,
                exchange_b: Exchange::Bitso,
            }],
        );

        // Bitso sells USDT/COP cheaper than Binance buys it
        let state = MarketState {
            order_books: vec![
                make_book(Exchange::Binance, TradingPair::UsdtCop, "4220", "4225"),
                make_book(Exchange::Bitso, TradingPair::UsdtCop, "4175", "4180"),
            ],
            tickers: vec![],
        };

        let signals = strategy.evaluate(&state).await;
        // Buy on Bitso at 4180, sell on Binance at 4220 = ~0.95% spread
        assert!(
            !signals.is_empty(),
            "Should detect cross-exchange opportunity"
        );
        assert_eq!(signals.len(), 2, "Should produce 2 signals (buy + sell)");

        // First signal should be buy on Bitso (cheaper)
        assert_eq!(signals[0].exchange, Exchange::Bitso);
        assert_eq!(signals[0].side, Side::Buy);

        // Second signal should be sell on Binance (more expensive)
        assert_eq!(signals[1].exchange, Exchange::Binance);
        assert_eq!(signals[1].side, Side::Sell);
    }

    #[tokio::test]
    async fn test_no_opportunity_when_spread_too_small() {
        let strategy = CrossExchangeArbitrage::new(
            dec!(0.5), // need 0.5% profit
            dec!(0.001),
            dec!(0.006),
            dec!(100),
            vec![CrossPairConfig {
                pair: TradingPair::UsdtCop,
                exchange_a: Exchange::Binance,
                exchange_b: Exchange::Bitso,
            }],
        );

        // Prices almost identical
        let state = MarketState {
            order_books: vec![
                make_book(Exchange::Binance, TradingPair::UsdtCop, "4200", "4201"),
                make_book(Exchange::Bitso, TradingPair::UsdtCop, "4199", "4200"),
            ],
            tickers: vec![],
        };

        let signals = strategy.evaluate(&state).await;
        assert!(
            signals.is_empty(),
            "Should not find opportunity with tight spread"
        );
    }
}
