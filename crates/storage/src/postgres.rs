use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::info;

use cripton_core::Trade;

/// PostgreSQL storage for trade logs and audit trail
pub struct PgStorage {
    pool: PgPool,
}

impl PgStorage {
    /// Connect to PostgreSQL and run migrations
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;

        info!("Connected to PostgreSQL");

        // Create tables if they don't exist
        Self::migrate(&pool).await?;

        Ok(Self { pool })
    }

    async fn migrate(pool: &PgPool) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS trades (
                id TEXT PRIMARY KEY,
                order_id TEXT NOT NULL,
                exchange TEXT NOT NULL,
                pair TEXT NOT NULL,
                side TEXT NOT NULL,
                price NUMERIC NOT NULL,
                quantity NUMERIC NOT NULL,
                fee NUMERIC NOT NULL,
                fee_currency TEXT NOT NULL,
                timestamp TIMESTAMPTZ NOT NULL,
                created_at TIMESTAMPTZ DEFAULT NOW()
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS daily_pnl (
                id SERIAL PRIMARY KEY,
                date DATE NOT NULL UNIQUE,
                total_trades INTEGER NOT NULL DEFAULT 0,
                total_volume NUMERIC NOT NULL DEFAULT 0,
                total_fees NUMERIC NOT NULL DEFAULT 0,
                net_pnl NUMERIC NOT NULL DEFAULT 0,
                updated_at TIMESTAMPTZ DEFAULT NOW()
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS circuit_breaker_events (
                id SERIAL PRIMARY KEY,
                event_type TEXT NOT NULL,
                reason TEXT NOT NULL,
                window_pnl NUMERIC,
                timestamp TIMESTAMPTZ DEFAULT NOW()
            )
            "#,
        )
        .execute(pool)
        .await?;

        info!("Database migrations complete");
        Ok(())
    }

    /// Record a completed trade
    pub async fn insert_trade(&self, trade: &Trade) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO trades (id, order_id, exchange, pair, side, price, quantity, fee, fee_currency, timestamp)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(&trade.id)
        .bind(&trade.order_id)
        .bind(format!("{}", trade.exchange))
        .bind(format!("{}", trade.pair))
        .bind(format!("{:?}", trade.side))
        .bind(trade.price)
        .bind(trade.quantity)
        .bind(trade.fee)
        .bind(&trade.fee_currency)
        .bind(trade.timestamp)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Record a batch of trades
    pub async fn insert_trades(&self, trades: &[Trade]) -> Result<()> {
        for trade in trades {
            self.insert_trade(trade).await?;
        }
        Ok(())
    }

    /// Record a circuit breaker event
    pub async fn record_circuit_breaker(
        &self,
        event_type: &str,
        reason: &str,
        window_pnl: rust_decimal::Decimal,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO circuit_breaker_events (event_type, reason, window_pnl)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(event_type)
        .bind(reason)
        .bind(window_pnl)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get total trades count
    pub async fn total_trades(&self) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    /// Get today's P&L summary
    pub async fn today_summary(&self) -> Result<(i64, rust_decimal::Decimal, rust_decimal::Decimal)> {
        let row: (i64, rust_decimal::Decimal, rust_decimal::Decimal) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*),
                COALESCE(SUM(quantity * price), 0),
                COALESCE(SUM(fee), 0)
            FROM trades
            WHERE timestamp::date = CURRENT_DATE
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row)
    }
}
