#!/usr/bin/env python3
"""
Cripton Backtesting Engine

Downloads historical kline/orderbook data from Binance and Bitso,
then simulates the triangular and cross-exchange arbitrage strategies.

Usage:
    pip install requests pandas
    python scripts/backtest.py --days 7 --strategy triangular
    python scripts/backtest.py --days 30 --strategy cross_exchange
    python scripts/backtest.py --days 7 --strategy all
"""

import argparse
import json
import sys
from datetime import datetime, timedelta
from decimal import Decimal, getcontext
from pathlib import Path

getcontext().prec = 18

try:
    import pandas as pd
    import requests
except ImportError:
    print("Install dependencies: pip install requests pandas")
    sys.exit(1)

# --- Configuration ---
BINANCE_API = "https://api.binance.com/api/v3"
BITSO_API = "https://bitso.com/api/v3"

BINANCE_FEE = Decimal("0.001")   # 0.1%
BITSO_FEE = Decimal("0.006")     # 0.6%
TRADE_AMOUNT = Decimal("100")     # $100 per trade
MIN_PROFIT_TRIANGULAR = Decimal("0.03")  # 0.03%
MIN_PROFIT_CROSS = Decimal("0.1")        # 0.1%


import time as _time

# SEC: rate limit for Binance API (max ~1200 weight/min, each kline = 1 weight)
_last_request_time = 0.0
_REQUEST_INTERVAL = 0.15  # 150ms between requests = ~400 req/min (safe margin)


def _rate_limited_get(url: str, params: dict) -> requests.Response:
    """Make a rate-limited GET request to avoid IP bans."""
    global _last_request_time
    elapsed = _time.time() - _last_request_time
    if elapsed < _REQUEST_INTERVAL:
        _time.sleep(_REQUEST_INTERVAL - elapsed)
    _last_request_time = _time.time()

    resp = requests.get(url, params=params, timeout=10)
    # Check for rate limit response
    if resp.status_code == 429:
        wait = int(resp.headers.get("Retry-After", "60"))
        print(f"  Rate limited! Waiting {wait}s...")
        _time.sleep(wait)
        return _rate_limited_get(url, params)  # retry
    resp.raise_for_status()
    return resp


def fetch_binance_klines(symbol: str, interval: str, days: int) -> pd.DataFrame:
    """Fetch historical klines from Binance with rate limiting."""
    # SEC: clamp days to prevent excessive API calls
    days = max(1, min(days, 365))

    end_time = int(datetime.now().timestamp() * 1000)
    start_time = int((datetime.now() - timedelta(days=days)).timestamp() * 1000)

    all_data = []
    request_count = 0
    while start_time < end_time:
        params = {
            "symbol": symbol,
            "interval": interval,
            "startTime": start_time,
            "limit": 1000,
        }
        resp = _rate_limited_get(f"{BINANCE_API}/klines", params)
        request_count += 1
        data = resp.json()
        if not data:
            break
        all_data.extend(data)
        start_time = data[-1][0] + 1

    if not all_data:
        return pd.DataFrame()

    df = pd.DataFrame(all_data, columns=[
        "open_time", "open", "high", "low", "close", "volume",
        "close_time", "quote_volume", "trades", "taker_buy_base",
        "taker_buy_quote", "ignore"
    ])
    df["open_time"] = pd.to_datetime(df["open_time"], unit="ms")
    df["close"] = df["close"].astype(float)
    df["open"] = df["open"].astype(float)
    df["high"] = df["high"].astype(float)
    df["low"] = df["low"].astype(float)
    df["volume"] = df["volume"].astype(float)
    return df


def backtest_triangular(days: int) -> dict:
    """Backtest triangular arbitrage: USDT → EURC → USDC → USDT"""
    print(f"\n=== Triangular Arbitrage Backtest ({days} days) ===\n")

    # Fetch data for all 3 legs
    print("Fetching EURCUSDT klines...")
    eurc_usdt = fetch_binance_klines("EURCUSDT", "1m", days)
    print("Fetching EURCUSDC klines...")
    eurc_usdc = fetch_binance_klines("EURCUSDC", "1m", days)
    print("Fetching USDTUSDC klines...")
    usdt_usdc = fetch_binance_klines("USDTUSDC", "1m", days)

    if eurc_usdt.empty or eurc_usdc.empty or usdt_usdc.empty:
        print("ERROR: could not fetch all pairs")
        return {"error": "missing data"}

    # Align timestamps
    merged = eurc_usdt[["open_time", "close"]].rename(columns={"close": "eurc_usdt"})
    merged = merged.merge(
        eurc_usdc[["open_time", "close"]].rename(columns={"close": "eurc_usdc"}),
        on="open_time", how="inner"
    )
    merged = merged.merge(
        usdt_usdc[["open_time", "close"]].rename(columns={"close": "usdt_usdc"}),
        on="open_time", how="inner"
    )

    print(f"Data points: {len(merged)}")

    fee_mult = float(1 - float(BINANCE_FEE))
    min_profit = float(MIN_PROFIT_TRIANGULAR)
    opportunities = 0
    total_profit = 0.0
    trades_log = []

    for _, row in merged.iterrows():
        p1 = row["eurc_usdt"]  # buy EURC with USDT (ask)
        p2 = row["eurc_usdc"]  # sell EURC for USDC (bid)
        p3 = row["usdt_usdc"]  # buy USDT with USDC (ask)

        if p1 <= 0 or p2 <= 0 or p3 <= 0:
            continue

        # Forward: USDT → EURC → USDC → USDT
        amount = 1.0
        amount = (amount / p1) * fee_mult   # buy EURC
        amount = (amount * p2) * fee_mult   # sell EURC for USDC
        amount = (amount / p3) * fee_mult   # buy USDT with USDC
        profit_fwd = (amount - 1.0) * 100

        # Reverse: USDT → USDC → EURC → USDT
        amount = 1.0
        amount = (amount * p3) * fee_mult   # sell USDT for USDC
        amount = (amount / p2) * fee_mult   # buy EURC with USDC
        amount = (amount * p1) * fee_mult   # sell EURC for USDT
        profit_rev = (amount - 1.0) * 100

        best_profit = max(profit_fwd, profit_rev)
        direction = "FWD" if profit_fwd >= profit_rev else "REV"

        if best_profit > min_profit:
            opportunities += 1
            trade_profit = float(TRADE_AMOUNT) * best_profit / 100
            total_profit += trade_profit
            trades_log.append({
                "time": str(row["open_time"]),
                "direction": direction,
                "profit_pct": round(best_profit, 4),
                "profit_usd": round(trade_profit, 4),
            })

    print(f"\nResults:")
    print(f"  Opportunities found: {opportunities}")
    print(f"  Total profit: ${total_profit:.2f}")
    print(f"  Avg profit per trade: ${total_profit / max(opportunities, 1):.4f}")
    print(f"  Trades per day: {opportunities / max(days, 1):.1f}")
    print(f"  Win rate: {opportunities / max(len(merged), 1) * 100:.2f}%")

    if trades_log:
        print(f"\n  Last 5 trades:")
        for t in trades_log[-5:]:
            print(f"    {t['time']} | {t['direction']} | {t['profit_pct']}% | ${t['profit_usd']}")

    return {
        "strategy": "triangular",
        "days": days,
        "data_points": len(merged),
        "opportunities": opportunities,
        "total_profit_usd": round(total_profit, 2),
        "avg_profit_per_trade": round(total_profit / max(opportunities, 1), 4),
        "trades_per_day": round(opportunities / max(days, 1), 1),
    }


def backtest_cross_exchange(days: int) -> dict:
    """
    Backtest cross-exchange COP arbitrage.
    Note: Bitso doesn't provide historical klines, so we simulate
    with a spread model based on known COP/USDT behavior.
    """
    print(f"\n=== Cross-Exchange COP Arbitrage Backtest ({days} days) ===\n")

    # We use Binance USDTCOP proxy (if available) or simulate
    # Bitso spread from known market behavior
    print("Fetching USDT price data from Binance...")
    usdt_data = fetch_binance_klines("USDTUSDC", "5m", days)

    if usdt_data.empty:
        print("ERROR: could not fetch data")
        return {"error": "missing data"}

    print(f"Data points: {len(usdt_data)}")

    # Simulate COP spread: Bitso typically trades 0.3-1.5% below/above Binance
    # This is a conservative model based on observed market behavior
    import random
    random.seed(42)  # reproducible results

    binance_fee = float(BINANCE_FEE)
    bitso_fee = float(BITSO_FEE)
    min_profit = float(MIN_PROFIT_CROSS)

    opportunities = 0
    total_profit = 0.0

    base_cop_rate = 4200.0  # approximate USDT/COP rate

    for _, row in usdt_data.iterrows():
        # Simulate Bitso and Binance COP prices with realistic spread
        volatility = random.gauss(0, 0.005)  # 0.5% std dev
        spread = random.uniform(0.002, 0.015)  # 0.2% to 1.5% spread

        binance_ask = base_cop_rate * (1 + volatility + spread / 2)
        binance_bid = base_cop_rate * (1 + volatility + spread / 2 - 0.001)
        bitso_ask = base_cop_rate * (1 + volatility - spread / 2)
        bitso_bid = base_cop_rate * (1 + volatility - spread / 2 - 0.002)

        # Direction 1: Buy on Bitso, Sell on Binance
        revenue = binance_bid * (1 - binance_fee)
        cost = bitso_ask / (1 - bitso_fee)
        profit_pct_1 = (revenue - cost) / cost * 100

        # Direction 2: Buy on Binance, Sell on Bitso
        revenue2 = bitso_bid * (1 - bitso_fee)
        cost2 = binance_ask / (1 - binance_fee)
        profit_pct_2 = (revenue2 - cost2) / cost2 * 100

        best = max(profit_pct_1, profit_pct_2)

        if best > min_profit:
            opportunities += 1
            trade_profit = float(TRADE_AMOUNT) * best / 100
            total_profit += trade_profit

    print(f"\nResults (simulated COP spread model):")
    print(f"  Opportunities found: {opportunities}")
    print(f"  Total profit: ${total_profit:.2f}")
    print(f"  Avg profit per trade: ${total_profit / max(opportunities, 1):.4f}")
    print(f"  Trades per day: {opportunities / max(days, 1):.1f}")
    print(f"  Win rate: {opportunities / max(len(usdt_data), 1) * 100:.2f}%")

    return {
        "strategy": "cross_exchange_cop",
        "days": days,
        "data_points": len(usdt_data),
        "opportunities": opportunities,
        "total_profit_usd": round(total_profit, 2),
        "avg_profit_per_trade": round(total_profit / max(opportunities, 1), 4),
        "trades_per_day": round(opportunities / max(days, 1), 1),
        "note": "simulated COP spread model (no Bitso historical API)",
    }


def main():
    parser = argparse.ArgumentParser(description="Cripton Backtesting Engine")
    parser.add_argument(
        "--days", type=int, default=7,
        help="Days of historical data (1-365)",
        choices=range(1, 366), metavar="DAYS",
    )
    parser.add_argument(
        "--strategy",
        choices=["triangular", "cross_exchange", "all"],
        default="all",
        help="Strategy to backtest",
    )
    parser.add_argument("--output", type=str, help="Save results to JSON file")
    args = parser.parse_args()

    results = []

    if args.strategy in ("triangular", "all"):
        results.append(backtest_triangular(args.days))

    if args.strategy in ("cross_exchange", "all"):
        results.append(backtest_cross_exchange(args.days))

    if args.output:
        Path(args.output).write_text(json.dumps(results, indent=2))
        print(f"\nResults saved to {args.output}")

    # Overall summary
    if len(results) > 1:
        total = sum(r.get("total_profit_usd", 0) for r in results)
        total_opps = sum(r.get("opportunities", 0) for r in results)
        print(f"\n=== OVERALL ===")
        print(f"  Total opportunities: {total_opps}")
        print(f"  Total profit: ${total:.2f}")
        print(f"  Daily average: ${total / max(args.days, 1):.2f}/day")


if __name__ == "__main__":
    main()
