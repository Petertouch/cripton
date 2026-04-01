use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::enums::{Exchange, OrderStatus, OrderType, Side, TradingPair};

/// A single price level in the order book
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub quantity: Decimal,
}

/// Snapshot of an order book for a single pair on a single exchange
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    pub exchange: Exchange,
    pub pair: TradingPair,
    pub bids: Vec<PriceLevel>,  // sorted descending by price (best bid first)
    pub asks: Vec<PriceLevel>,  // sorted ascending by price (best ask first)
    pub timestamp: DateTime<Utc>,
}

impl OrderBook {
    pub fn best_bid(&self) -> Option<&PriceLevel> {
        self.bids.first()
    }

    pub fn best_ask(&self) -> Option<&PriceLevel> {
        self.asks.first()
    }

    /// Spread between best ask and best bid
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_ask(), self.best_bid()) {
            (Some(ask), Some(bid)) => Some(ask.price - bid.price),
            _ => None,
        }
    }

    /// Spread as a percentage of the mid price
    pub fn spread_pct(&self) -> Option<Decimal> {
        match (self.best_ask(), self.best_bid()) {
            (Some(ask), Some(bid)) => {
                let mid = (ask.price + bid.price) / Decimal::TWO;
                if mid.is_zero() {
                    None
                } else {
                    Some((ask.price - bid.price) / mid * Decimal::ONE_HUNDRED)
                }
            }
            _ => None,
        }
    }
}

/// Real-time ticker for a trading pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticker {
    pub exchange: Exchange,
    pub pair: TradingPair,
    pub bid: Decimal,
    pub ask: Decimal,
    pub last_price: Decimal,
    pub volume_24h: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// An order to be placed on an exchange
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub exchange: Exchange,
    pub pair: TradingPair,
    pub side: Side,
    pub order_type: OrderType,
    pub price: Option<Decimal>,  // None for market orders
    pub quantity: Decimal,
    pub status: OrderStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A completed trade
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: String,
    pub order_id: String,
    pub exchange: Exchange,
    pub pair: TradingPair,
    pub side: Side,
    pub price: Decimal,
    pub quantity: Decimal,
    pub fee: Decimal,
    pub fee_currency: String,
    pub timestamp: DateTime<Utc>,
}

/// Signal emitted by a strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub exchange: Exchange,
    pub pair: TradingPair,
    pub side: Side,
    pub order_type: OrderType,
    pub price: Option<Decimal>,
    pub quantity: Decimal,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

/// Current state of the market aggregated across exchanges
#[derive(Debug, Clone, Default)]
pub struct MarketState {
    pub order_books: Vec<OrderBook>,
    pub tickers: Vec<Ticker>,
}

impl MarketState {
    pub fn get_orderbook(&self, exchange: Exchange, pair: TradingPair) -> Option<&OrderBook> {
        self.order_books
            .iter()
            .find(|ob| ob.exchange == exchange && ob.pair == pair)
    }

    pub fn get_ticker(&self, exchange: Exchange, pair: TradingPair) -> Option<&Ticker> {
        self.tickers
            .iter()
            .find(|t| t.exchange == exchange && t.pair == pair)
    }
}
