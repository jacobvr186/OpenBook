use ordered_float::OrderedFloat;
use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct ProfilingStats {
    pub ws_parse_time: AtomicU64,
    pub ws_parse_count: AtomicU64,
    pub orderbook_update_time: AtomicU64,
    pub orderbook_update_count: AtomicU64,
    pub lock_acquire_time: AtomicU64,
    pub lock_acquire_count: AtomicU64,
    pub render_time: AtomicU64,
    pub render_count: AtomicU64,
    pub aggregation_time: AtomicU64,
    pub aggregation_count: AtomicU64,
    pub network_latency: AtomicU64,
    pub network_count: AtomicU64,
    pub processing_latency: AtomicU64,
    pub processing_count: AtomicU64,
}

impl ProfilingStats {
    pub fn record(&self, field: &AtomicU64, count_field: &AtomicU64, micros: u64) {
        field.fetch_add(micros, Ordering::Relaxed);
        count_field.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Deserialize)]
pub struct RestDepthResponse {
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: u64,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
}

#[derive(Debug, Deserialize)]
pub struct ServerTimeResponse {
    #[serde(rename = "serverTime")]
    pub server_time: u64,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct WsDepthUpdate {
    #[serde(rename = "e")]
    pub event_type: String,
    #[serde(rename = "E")]
    pub event_time: u64,
    #[serde(rename = "T")]
    pub transaction_time: u64,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "U")]
    pub first_update_id: u64,
    #[serde(rename = "u")]
    pub final_update_id: u64,
    #[serde(rename = "pu")]
    pub prev_final_update_id: u64,
    #[serde(rename = "b")]
    pub bids: Vec<[String; 2]>,
    #[serde(rename = "a")]
    pub asks: Vec<[String; 2]>,
}

#[derive(Clone)]
pub struct OrderBook {
    pub bids: BTreeMap<OrderedFloat<f64>, f64>,
    pub asks: BTreeMap<OrderedFloat<f64>, f64>,
    pub last_update_id: u64,
    pub last_event_time: u64,
    pub clock_offset: i64,
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_update_id: 0,
            last_event_time: 0,
            clock_offset: 0,
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: &RestDepthResponse) {
        self.bids.clear();
        self.asks.clear();
        for bid in &snapshot.bids {
            if let (Ok(p), Ok(q)) = (bid[0].parse::<f64>(), bid[1].parse::<f64>()) {
                if q > 0.0 {
                    self.bids.insert(OrderedFloat(p), q);
                }
            }
        }
        for ask in &snapshot.asks {
            if let (Ok(p), Ok(q)) = (ask[0].parse::<f64>(), ask[1].parse::<f64>()) {
                if q > 0.0 {
                    self.asks.insert(OrderedFloat(p), q);
                }
            }
        }
        self.last_update_id = snapshot.last_update_id;
    }

    pub fn apply_update(&mut self, update: &WsDepthUpdate) {
        for bid in &update.bids {
            if let (Ok(p), Ok(q)) = (bid[0].parse::<f64>(), bid[1].parse::<f64>()) {
                let key = OrderedFloat(p);
                if q == 0.0 {
                    self.bids.remove(&key);
                } else {
                    self.bids.insert(key, q);
                }
            }
        }
        for ask in &update.asks {
            if let (Ok(p), Ok(q)) = (ask[0].parse::<f64>(), ask[1].parse::<f64>()) {
                let key = OrderedFloat(p);
                if q == 0.0 {
                    self.asks.remove(&key);
                } else {
                    self.asks.insert(key, q);
                }
            }
        }
        self.last_update_id = update.final_update_id;
        self.last_event_time = update.event_time;
    }

    pub fn best_bid(&self) -> Option<(f64, f64)> {
        self.bids.iter().next_back().map(|(p, q)| (p.0, *q))
    }

    pub fn best_ask(&self) -> Option<(f64, f64)> {
        self.asks.iter().next().map(|(p, q)| (p.0, *q))
    }

    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some(ask - bid),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct WsAggTrade {
    #[serde(rename = "e")]
    pub event_type: String,
    #[serde(rename = "E")]
    pub event_time: u64,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "a")]
    pub agg_trade_id: u64,
    #[serde(rename = "p")]
    pub price: String,
    #[serde(rename = "q")]
    pub quantity: String,
    #[serde(rename = "f")]
    pub first_trade_id: u64,
    #[serde(rename = "l")]
    pub last_trade_id: u64,
    #[serde(rename = "T")]
    pub trade_time: u64,
    #[serde(rename = "m")]
    pub is_buyer_maker: bool,
}

#[derive(Clone, Debug)]
pub struct Trade {
    pub timestamp_ms: u64,
    pub price: f64,
    pub quantity: f64,
    pub is_buy: bool,
}

#[derive(Clone)]
pub struct TradeHistory {
    pub price_points: VecDeque<(u64, f64)>,
    pub trades: VecDeque<Trade>,
    pub window_ms: u64,
}

impl TradeHistory {
    pub fn new(window_ms: u64) -> Self {
        Self {
            price_points: VecDeque::new(),
            trades: VecDeque::new(),
            window_ms,
        }
    }

    pub fn add_price(&mut self, timestamp_ms: u64, price: f64) {
        self.price_points.push_back((timestamp_ms, price));
        self.cleanup(timestamp_ms);
    }

    pub fn add_trade(&mut self, trade: Trade) {
        let timestamp = trade.timestamp_ms;
        self.trades.push_back(trade);
        self.cleanup(timestamp);
    }

    fn cleanup(&mut self, current_time_ms: u64) {
        let cutoff = current_time_ms.saturating_sub(self.window_ms);
        while let Some(&(ts, _)) = self.price_points.front() {
            if ts < cutoff {
                self.price_points.pop_front();
            } else {
                break;
            }
        }
        while let Some(trade) = self.trades.front() {
            if trade.timestamp_ms < cutoff {
                self.trades.pop_front();
            } else {
                break;
            }
        }
    }
}

pub struct App {
    pub symbol: String,
    pub bucket_size: f64,
    pub render_time_us: u64,
}

impl App {
    pub fn new(symbol: String, bucket_size: f64) -> Self {
        Self {
            symbol,
            bucket_size,
            render_time_us: 0,
        }
    }
}
