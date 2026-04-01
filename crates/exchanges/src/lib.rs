pub mod binance;
pub mod bitso;
pub mod traits;

pub use binance::BinanceClient;
pub use bitso::BitsoClient;
pub use traits::{ExchangeConnector, MarketEvent};
