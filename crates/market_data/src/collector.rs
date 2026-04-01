use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{RwLock, mpsc};
use tracing::{info, warn};

use cripton_core::{MarketState, TradingPair};
use cripton_exchanges::{ExchangeConnector, MarketEvent};

use crate::normalizer::Normalizer;
use crate::orderbook::OrderBookCache;

/// Central market data collector.
/// Connects to exchanges via WebSocket and maintains an up-to-date MarketState.
pub struct Collector {
    exchanges: Vec<Arc<dyn ExchangeConnector>>,
    pairs: Vec<TradingPair>,
    cache: Arc<RwLock<OrderBookCache>>,
}

impl Collector {
    pub fn new(exchanges: Vec<Arc<dyn ExchangeConnector>>, pairs: Vec<TradingPair>) -> Self {
        Self {
            exchanges,
            pairs,
            cache: Arc::new(RwLock::new(OrderBookCache::new())),
        }
    }

    /// Start collecting data from all exchanges.
    /// Returns a handle to read the current market state.
    pub async fn start(&self) -> Result<mpsc::UnboundedReceiver<MarketState>> {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<MarketEvent>();
        let (state_tx, state_rx) = mpsc::unbounded_channel::<MarketState>();

        // Subscribe to each exchange's WebSocket
        for exchange in &self.exchanges {
            info!("Subscribing to {} order books", exchange.exchange());
            if let Err(e) = exchange
                .subscribe_orderbook(&self.pairs, event_tx.clone())
                .await
            {
                warn!("Failed to subscribe to {}: {}", exchange.exchange(), e);
            }
        }

        // Fetch initial snapshots via REST
        for exchange in &self.exchanges {
            for pair in &self.pairs {
                let exchange = exchange.clone();
                let pair = *pair;
                let cache = self.cache.clone();
                tokio::spawn(async move {
                    match exchange.fetch_orderbook(pair).await {
                        Ok(book) => {
                            // SEC: validate before caching
                            if book.is_valid() {
                                cache.write().await.update(book);
                                info!("Initial snapshot loaded: {} {}", exchange.exchange(), pair);
                            } else {
                                warn!(
                                    "Rejected invalid initial snapshot: {} {}",
                                    exchange.exchange(),
                                    pair
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Failed to fetch initial {} {}: {}",
                                exchange.exchange(),
                                pair,
                                e
                            );
                        }
                    }
                });
            }
        }

        // Process incoming events and emit updated MarketState
        let cache = self.cache.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    MarketEvent::OrderBookUpdate(book) => {
                        // SEC: validate order book before accepting it
                        if !book.is_valid() {
                            warn!(
                                "Rejected invalid order book update: {} {}",
                                book.exchange, book.pair
                            );
                            continue;
                        }

                        // SEC: minimize write lock duration — update cache, clone data,
                        // then release lock BEFORE sending state downstream
                        let state = {
                            let mut cache_w = cache.write().await;
                            cache_w.update(book);
                            let books = cache_w.all().into_iter().cloned().collect();
                            Normalizer::build_state(books, vec![])
                        }; // write lock released here

                        if state_tx.send(state).is_err() {
                            warn!("State receiver dropped, stopping collector");
                            break;
                        }
                    }
                    MarketEvent::TickerUpdate(_ticker) => {}
                    MarketEvent::ConnectionLost(exchange) => {
                        warn!("Lost connection to {}", exchange);
                    }
                    MarketEvent::ConnectionRestored(exchange) => {
                        info!("Reconnected to {}", exchange);
                    }
                }
            }
        });

        Ok(state_rx)
    }

    /// Get a snapshot of the current market state
    pub async fn snapshot(&self) -> MarketState {
        let cache = self.cache.read().await;
        let books = cache.all().into_iter().cloned().collect();
        Normalizer::build_state(books, vec![])
    }
}
