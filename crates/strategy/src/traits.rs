use async_trait::async_trait;
use cripton_core::{MarketState, Signal};

/// Every trading strategy implements this trait
#[async_trait]
pub trait Strategy: Send + Sync {
    /// Human-readable name of the strategy
    fn name(&self) -> &str;

    /// Evaluate the current market state and return trading signals.
    /// Returns an empty vec if no opportunity is found.
    async fn evaluate(&self, state: &MarketState) -> Vec<Signal>;
}
