"use client";

import { useEffect, useState } from "react";

const API_BASE = process.env.NEXT_PUBLIC_API_URL || "http://localhost:3001/api";

interface Status {
  status: string;
  uptime_seconds: number;
  paper_mode: boolean;
  circuit_breaker_active: boolean;
  window_pnl: string;
  current_exposure: string;
  active_window: string | null;
  is_aggressive: boolean;
  filled_orders: number;
  total_volume: string;
}

interface Trades {
  total_trades: number | null;
  today_trades: number | null;
  today_volume: string | null;
  today_fees: string | null;
  db_connected: boolean;
}

function formatUptime(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  return `${h}h ${m}m ${s}s`;
}

function Card({
  label,
  value,
  alert,
}: {
  label: string;
  value: string;
  alert?: boolean;
}) {
  return (
    <div
      className={`rounded-lg p-4 ${alert ? "bg-red-900/50 border border-red-500" : "bg-zinc-800 border border-zinc-700"}`}
    >
      <p className="text-sm text-zinc-400">{label}</p>
      <p
        className={`text-xl font-mono ${alert ? "text-red-400" : "text-white"}`}
      >
        {value}
      </p>
    </div>
  );
}

export default function Dashboard() {
  const [status, setStatus] = useState<Status | null>(null);
  const [trades, setTrades] = useState<Trades | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdate, setLastUpdate] = useState<Date | null>(null);

  useEffect(() => {
    const fetchData = async () => {
      try {
        const [statusRes, tradesRes] = await Promise.all([
          fetch(`${API_BASE}/status`),
          fetch(`${API_BASE}/trades`),
        ]);
        if (statusRes.ok) setStatus(await statusRes.json());
        if (tradesRes.ok) setTrades(await tradesRes.json());
        setError(null);
        setLastUpdate(new Date());
      } catch {
        setError("Cannot connect to Cripton API");
      }
    };

    fetchData();
    const interval = setInterval(fetchData, 3000);
    return () => clearInterval(interval);
  }, []);

  return (
    <main className="min-h-screen bg-zinc-950 text-white p-6">
      <div className="max-w-5xl mx-auto">
        <div className="flex items-center justify-between mb-8">
          <div>
            <h1 className="text-3xl font-bold">Cripton Dashboard</h1>
            <p className="text-zinc-400 text-sm">Stablecoin Arbitrage Bot</p>
          </div>
          <div className="text-right flex gap-2">
            {status?.paper_mode && (
              <span className="bg-yellow-600 text-black px-3 py-1 rounded-full text-sm font-bold">
                PAPER
              </span>
            )}
            {error ? (
              <span className="bg-red-600 px-3 py-1 rounded-full text-sm">
                OFFLINE
              </span>
            ) : (
              <span className="bg-green-600 px-3 py-1 rounded-full text-sm">
                LIVE
              </span>
            )}
          </div>
        </div>

        {error && (
          <div className="bg-red-900/30 border border-red-500 rounded-lg p-4 mb-6">
            <p className="text-red-400">{error}</p>
            <p className="text-zinc-500 text-sm mt-1">
              Start the bot: RUST_LOG=info cargo run
            </p>
          </div>
        )}

        {status && (
          <>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
              <Card
                label="Status"
                value={status.status.replace(/_/g, " ").toUpperCase()}
                alert={status.circuit_breaker_active}
              />
              <Card
                label="Uptime"
                value={formatUptime(status.uptime_seconds)}
              />
              <Card label="Exposure" value={`$${status.current_exposure}`} />
              <Card
                label="Window P&L"
                value={`$${status.window_pnl}`}
                alert={parseFloat(status.window_pnl) < -10}
              />
            </div>

            <div className="grid grid-cols-2 md:grid-cols-3 gap-4 mb-6">
              <Card
                label="Active Window"
                value={status.active_window || "None"}
              />
              <Card
                label="Aggressive"
                value={status.is_aggressive ? "YES" : "No"}
                alert={status.is_aggressive}
              />
              <Card label="Filled Orders" value={`${status.filled_orders}`} />
            </div>

            {status.circuit_breaker_active && (
              <div className="bg-red-900/30 border border-red-500 rounded-lg p-4 mb-6">
                <h3 className="text-red-400 font-bold text-lg">
                  CIRCUIT BREAKER ACTIVE
                </h3>
                <p className="text-zinc-300 mt-1">
                  Trading halted. Window P&L: ${status.window_pnl}
                </p>
              </div>
            )}
          </>
        )}

        {trades && (
          <div className="bg-zinc-900 border border-zinc-700 rounded-lg p-6 mb-6">
            <h2 className="text-lg font-bold mb-4">Trades</h2>
            {trades.db_connected ? (
              <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
                <Card
                  label="Total"
                  value={trades.total_trades?.toString() ?? "0"}
                />
                <Card
                  label="Today"
                  value={trades.today_trades?.toString() ?? "0"}
                />
                <Card
                  label="Today Volume"
                  value={trades.today_volume ? `$${trades.today_volume}` : "$0"}
                />
                <Card
                  label="Today Fees"
                  value={trades.today_fees ? `$${trades.today_fees}` : "$0"}
                />
              </div>
            ) : (
              <p className="text-zinc-500">Database not connected</p>
            )}
          </div>
        )}

        <div className="text-center text-zinc-600 text-sm">
          {lastUpdate && <p>Updated: {lastUpdate.toLocaleTimeString()}</p>}
          <p className="mt-1">Auto-refresh 3s</p>
        </div>
      </div>
    </main>
  );
}
