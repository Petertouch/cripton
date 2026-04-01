-- Cripton database setup
-- Run: psql -U postgres -f scripts/setup_db.sql

CREATE DATABASE cripton;

\c cripton

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
);

CREATE TABLE IF NOT EXISTS daily_pnl (
    id SERIAL PRIMARY KEY,
    date DATE NOT NULL UNIQUE,
    total_trades INTEGER NOT NULL DEFAULT 0,
    total_volume NUMERIC NOT NULL DEFAULT 0,
    total_fees NUMERIC NOT NULL DEFAULT 0,
    net_pnl NUMERIC NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS circuit_breaker_events (
    id SERIAL PRIMARY KEY,
    event_type TEXT NOT NULL,
    reason TEXT NOT NULL,
    window_pnl NUMERIC,
    timestamp TIMESTAMPTZ DEFAULT NOW()
);

-- Indices for performance
CREATE INDEX IF NOT EXISTS idx_trades_timestamp ON trades(timestamp);
CREATE INDEX IF NOT EXISTS idx_trades_exchange_pair ON trades(exchange, pair);
CREATE INDEX IF NOT EXISTS idx_daily_pnl_date ON daily_pnl(date);
