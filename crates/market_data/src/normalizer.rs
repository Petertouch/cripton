use cripton_core::{MarketState, OrderBook, Ticker};

/// Builds a unified MarketState from cached data
pub struct Normalizer;

impl Normalizer {
    /// Convert raw order books and tickers into a unified MarketState
    pub fn build_state(order_books: Vec<OrderBook>, tickers: Vec<Ticker>) -> MarketState {
        MarketState {
            order_books,
            tickers,
        }
    }
}
