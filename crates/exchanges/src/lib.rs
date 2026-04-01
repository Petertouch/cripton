pub mod binance;
pub mod bitso;
pub mod kraken;
pub mod traits;

pub use binance::BinanceClient;
pub use bitso::BitsoClient;
pub use kraken::KrakenClient;
pub use traits::{ExchangeConnector, MarketEvent};
