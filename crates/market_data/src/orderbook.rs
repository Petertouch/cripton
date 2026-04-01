use std::collections::HashMap;

use cripton_core::{Exchange, OrderBook, TradingPair};

/// In-memory cache of the latest order books per exchange+pair
#[derive(Debug, Default)]
pub struct OrderBookCache {
    books: HashMap<(Exchange, TradingPair), OrderBook>,
}

impl OrderBookCache {
    pub fn new() -> Self {
        Self {
            books: HashMap::new(),
        }
    }

    pub fn update(&mut self, book: OrderBook) {
        self.books.insert((book.exchange, book.pair), book);
    }

    pub fn get(&self, exchange: Exchange, pair: TradingPair) -> Option<&OrderBook> {
        self.books.get(&(exchange, pair))
    }

    pub fn get_all_for_pair(&self, pair: TradingPair) -> Vec<&OrderBook> {
        self.books.values().filter(|ob| ob.pair == pair).collect()
    }

    pub fn all(&self) -> Vec<&OrderBook> {
        self.books.values().collect()
    }
}
