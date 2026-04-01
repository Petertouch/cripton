use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use uuid::Uuid;

use cripton_core::{Order, OrderStatus, OrderType, Signal};

/// Tracks all orders and their lifecycle
#[derive(Debug, Default)]
pub struct OrderManager {
    orders: HashMap<String, Order>,
}

impl OrderManager {
    pub fn new() -> Self {
        Self {
            orders: HashMap::new(),
        }
    }

    /// Create an Order from a Signal, assigning a local ID
    pub fn create_order(&mut self, signal: &Signal) -> Order {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        let order = Order {
            id: id.clone(),
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

        self.orders.insert(id, order.clone());
        order
    }

    /// Update the status of an order after exchange confirmation
    pub fn update_status(&mut self, order_id: &str, status: OrderStatus) {
        if let Some(order) = self.orders.get_mut(order_id) {
            order.status = status;
            order.updated_at = Utc::now();
        }
    }

    /// Update the exchange-assigned order ID
    pub fn set_exchange_id(&mut self, local_id: &str, exchange_id: &str) {
        if let Some(order) = self.orders.get_mut(local_id) {
            // Store exchange ID in the order — we reuse the id field
            // after submission to track with the exchange
            order.id = exchange_id.to_string();
            order.updated_at = Utc::now();
        }
    }

    /// Get all pending orders
    pub fn pending_orders(&self) -> Vec<&Order> {
        self.orders
            .values()
            .filter(|o| o.status == OrderStatus::Pending)
            .collect()
    }

    /// Get all orders
    pub fn all_orders(&self) -> Vec<&Order> {
        self.orders.values().collect()
    }

    /// Count of filled orders
    pub fn filled_count(&self) -> usize {
        self.orders
            .values()
            .filter(|o| o.status == OrderStatus::Filled)
            .count()
    }

    /// Total traded volume (filled orders only)
    pub fn total_volume(&self) -> Decimal {
        self.orders
            .values()
            .filter(|o| o.status == OrderStatus::Filled)
            .map(|o| o.quantity * o.price.unwrap_or_default())
            .sum()
    }
}
