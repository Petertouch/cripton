use thiserror::Error;

use crate::enums::Exchange;

#[derive(Error, Debug)]
pub enum CriptonError {
    #[error("Exchange error ({exchange}): {message}")]
    Exchange {
        exchange: Exchange,
        message: String,
    },

    #[error("WebSocket connection failed: {0}")]
    WebSocket(String),

    #[error("Order rejected: {0}")]
    OrderRejected(String),

    // SEC: do not expose exact balance amounts in error messages
    #[error("Insufficient balance for requested operation")]
    InsufficientBalance,

    #[error("Rate limited by {exchange}, retry after {retry_after_ms}ms")]
    RateLimited {
        exchange: Exchange,
        retry_after_ms: u64,
    },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Pair not supported on {exchange}: {pair}")]
    UnsupportedPair {
        exchange: Exchange,
        pair: String,
    },

    #[error("HTTP request failed")]
    Http(#[from] reqwest::Error),

    #[error("JSON parsing failed")]
    SerdeJson(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, CriptonError>;
