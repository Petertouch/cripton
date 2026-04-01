use anyhow::Result;
use async_trait::async_trait;
use cripton_core::{Exchange, OrderBook, Side, Ticker, TradingPair};
use rust_decimal::Decimal;
use tokio::sync::mpsc;

/// Event emitted by an exchange connector
#[derive(Debug, Clone)]
pub enum MarketEvent {
    OrderBookUpdate(OrderBook),
    TickerUpdate(Ticker),
    ConnectionLost(Exchange),
    ConnectionRestored(Exchange),
}

/// Trait that every exchange connector must implement
#[async_trait]
pub trait ExchangeConnector: Send + Sync {
    /// Which exchange this connector is for
    fn exchange(&self) -> Exchange;

    /// Fetch the current order book via REST
    async fn fetch_orderbook(&self, pair: TradingPair) -> Result<OrderBook>;

    /// Fetch the current ticker via REST
    async fn fetch_ticker(&self, pair: TradingPair) -> Result<Ticker>;

    /// Subscribe to real-time order book updates via WebSocket.
    /// Sends events through the provided channel.
    async fn subscribe_orderbook(
        &self,
        pairs: &[TradingPair],
        tx: mpsc::UnboundedSender<MarketEvent>,
    ) -> Result<()>;

    /// Place a limit order, returns the exchange order ID
    async fn place_limit_order(
        &self,
        pair: TradingPair,
        side: Side,
        price: Decimal,
        quantity: Decimal,
    ) -> Result<String>;

    /// Place a market order, returns the exchange order ID
    async fn place_market_order(
        &self,
        pair: TradingPair,
        side: Side,
        quantity: Decimal,
    ) -> Result<String>;

    /// Cancel an order by its exchange ID
    async fn cancel_order(&self, pair: TradingPair, order_id: &str) -> Result<()>;

    /// Get available balance for a given asset (e.g. "USDT")
    async fn get_balance(&self, asset: &str) -> Result<Decimal>;
}
