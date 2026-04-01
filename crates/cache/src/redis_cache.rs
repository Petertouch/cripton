use anyhow::{Context, Result};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use tracing::{info, warn};

use cripton_core::{Exchange, OrderBook, TradingPair};

const KEY_PREFIX: &str = "cripton:orderbook";
const TTL_SECONDS: u64 = 10; // order books expire after 10s

/// Redis-backed order book cache for sub-millisecond reads.
///
/// Falls back gracefully if Redis is unavailable — the bot continues
/// with the in-memory RwLock cache in market_data::Collector.
pub struct RedisOrderBookCache {
    conn: ConnectionManager,
}

impl RedisOrderBookCache {
    pub async fn new(redis_url: &str) -> Result<Self> {
        let client = redis::Client::open(redis_url).context("Invalid Redis URL")?;
        let conn = ConnectionManager::new(client)
            .await
            .context("Failed to connect to Redis")?;

        info!("Redis cache connected");
        Ok(Self { conn })
    }

    fn key(exchange: Exchange, pair: TradingPair) -> String {
        format!("{}:{}:{}", KEY_PREFIX, exchange, pair)
    }

    /// Store an order book in Redis with TTL
    pub async fn set(&self, book: &OrderBook) -> Result<()> {
        let key = Self::key(book.exchange, book.pair);
        let json = serde_json::to_string(book)?;

        let mut conn = self.conn.clone();
        conn.set_ex::<_, _, ()>(&key, &json, TTL_SECONDS).await?;

        Ok(())
    }

    /// Fetch an order book from Redis (returns None if missing or expired)
    pub async fn get(&self, exchange: Exchange, pair: TradingPair) -> Option<OrderBook> {
        let key = Self::key(exchange, pair);
        let mut conn = self.conn.clone();

        match conn.get::<_, Option<String>>(&key).await {
            Ok(Some(json)) => match serde_json::from_str::<OrderBook>(&json) {
                Ok(book) => Some(book),
                Err(e) => {
                    warn!("Redis: failed to deserialize order book: {}", e);
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                warn!("Redis: GET failed: {}", e);
                None
            }
        }
    }

    /// Store multiple order books
    pub async fn set_many(&self, books: &[OrderBook]) -> Result<()> {
        for book in books {
            self.set(book).await?;
        }
        Ok(())
    }

    /// Check if Redis is alive
    pub async fn ping(&self) -> bool {
        let mut conn = self.conn.clone();
        redis::cmd("PING")
            .query_async::<bool>(&mut conn)
            .await
            .unwrap_or(false)
    }
}
