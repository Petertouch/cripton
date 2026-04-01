use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use uuid::Uuid;

use cripton_core::{Order, OrderStatus, Signal};

/// Tracks all orders and their lifecycle.
/// Orders are keyed by local_id (UUID) which never changes.
#[derive(Debug, Default)]
pub struct OrderManager {
    /// local_id -> Order
    orders: HashMap<String, Order>,
    /// exchange_id -> local_id (reverse lookup)
    exchange_to_local: HashMap<String, String>,
    /// Maximum orders to keep in memory before pruning completed ones
    max_orders: usize,
}

impl OrderManager {
    pub fn new() -> Self {
        Self {
            orders: HashMap::new(),
            exchange_to_local: HashMap::new(),
            max_orders: 10_000,
        }
    }

    /// Create an Order from a Signal, assigning a local ID
    pub fn create_order(&mut self, signal: &Signal) -> Order {
        let local_id = Uuid::new_v4().to_string();
        let now = Utc::now();

        let order = Order {
            local_id: local_id.clone(),
            exchange_id: None,
            exchange: signal.exchange,
            pair: signal.pair,
            side: signal.side,
            order_type: signal.order_type,
            price: signal.price,
            quantity: signal.quantity,
            status: OrderStatus::Pending,
            created_at: now,
            updated_at: now,
        };

        self.orders.insert(local_id, order.clone());
        self.prune_if_needed();
        order
    }

    /// Update the status of an order by local_id
    pub fn update_status(&mut self, local_id: &str, status: OrderStatus) {
        if let Some(order) = self.orders.get_mut(local_id) {
            order.status = status;
            order.updated_at = Utc::now();
        }
    }

    /// Associate the exchange-assigned order ID with a local order.
    /// Does NOT overwrite the local_id — stores exchange_id separately.
    pub fn set_exchange_id(&mut self, local_id: &str, exchange_id: &str) {
        if let Some(order) = self.orders.get_mut(local_id) {
            order.exchange_id = Some(exchange_id.to_string());
            order.updated_at = Utc::now();
            self.exchange_to_local
                .insert(exchange_id.to_string(), local_id.to_string());
        }
    }

    /// Look up an order by its exchange-assigned ID
    pub fn get_by_exchange_id(&self, exchange_id: &str) -> Option<&Order> {
        self.exchange_to_local
            .get(exchange_id)
            .and_then(|local_id| self.orders.get(local_id))
    }

    /// Get all pending orders
    pub fn pending_orders(&self) -> Vec<&Order> {
        self.orders
            .values()
            .filter(|o| o.status == OrderStatus::Pending)
            .collect()
    }

    /// Count of filled orders
    pub fn filled_count(&self) -> usize {
        self.orders
            .values()
            .filter(|o| o.status == OrderStatus::Filled)
            .count()
    }

    /// Total traded volume (filled orders with known prices only)
    pub fn total_volume(&self) -> Decimal {
        self.orders
            .values()
            .filter(|o| o.status == OrderStatus::Filled && o.price.is_some())
            .map(|o| o.quantity * o.price.unwrap_or_default())
            .sum()
    }

    /// Prune old completed orders to prevent memory bloat
    fn prune_if_needed(&mut self) {
        if self.orders.len() <= self.max_orders {
            return;
        }

        // Remove oldest completed orders
        let mut completed: Vec<(String, chrono::DateTime<Utc>)> = self
            .orders
            .iter()
            .filter(|(_, o)| {
                o.status == OrderStatus::Filled
                    || o.status == OrderStatus::Cancelled
                    || o.status == OrderStatus::Rejected
            })
            .map(|(id, o)| (id.clone(), o.updated_at))
            .collect();

        completed.sort_by_key(|(_, ts)| *ts);

        // Remove oldest half of completed orders
        let to_remove = completed.len() / 2;
        for (local_id, _) in completed.into_iter().take(to_remove) {
            if let Some(order) = self.orders.remove(&local_id) {
                if let Some(eid) = &order.exchange_id {
                    self.exchange_to_local.remove(eid);
                }
            }
        }
    }
}
