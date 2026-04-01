use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Exchange {
    Binance,
    Bitso,
    Kraken,
}

impl fmt::Display for Exchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Exchange::Binance => write!(f, "Binance"),
            Exchange::Bitso => write!(f, "Bitso"),
            Exchange::Kraken => write!(f, "Kraken"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderStatus {
    Pending,
    Filled,
    PartiallyFilled,
    Cancelled,
    Rejected,
}

/// Supported trading pairs — stablecoin focused
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TradingPair {
    UsdtUsdc,
    UsdtEurc,
    EurcUsdc,
    UsdtCop,    // via Bitso
    UsdcCop,
    EurUsdt,
    EurUsdc,
}

impl TradingPair {
    pub fn as_binance_symbol(&self) -> Option<&'static str> {
        match self {
            TradingPair::UsdtUsdc => Some("USDTUSDC"),
            TradingPair::UsdtEurc => Some("EURCUSDT"),
            TradingPair::EurcUsdc => Some("EURCUSDC"),
            TradingPair::EurUsdt => Some("EURUSDT"),
            TradingPair::EurUsdc => Some("EURUSDC"),
            _ => None,
        }
    }

    pub fn base(&self) -> &'static str {
        match self {
            TradingPair::UsdtUsdc => "USDT",
            TradingPair::UsdtEurc => "USDT",
            TradingPair::EurcUsdc => "EURC",
            TradingPair::UsdtCop => "USDT",
            TradingPair::UsdcCop => "USDC",
            TradingPair::EurUsdt => "EUR",
            TradingPair::EurUsdc => "EUR",
        }
    }

    pub fn quote(&self) -> &'static str {
        match self {
            TradingPair::UsdtUsdc => "USDC",
            TradingPair::UsdtEurc => "EURC",
            TradingPair::EurcUsdc => "USDC",
            TradingPair::UsdtCop => "COP",
            TradingPair::UsdcCop => "COP",
            TradingPair::EurUsdt => "USDT",
            TradingPair::EurUsdc => "USDC",
        }
    }
}

impl fmt::Display for TradingPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.base(), self.quote())
    }
}
