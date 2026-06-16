use std::sync::OnceLock;

use prometheus::{
    Counter, CounterVec, Histogram,
    Opts, HistogramOpts, Registry,
};

pub static REGISTRY: OnceLock<Registry> = OnceLock::new();
pub static REQ_COUNTER: OnceLock<CounterVec> = OnceLock::new();
pub static REQ_DURATION: OnceLock<Histogram> = OnceLock::new();
pub static ACTIVE_CONNS: OnceLock<Counter> = OnceLock::new();

pub fn init() {
    let registry = Registry::new();

    let req_counter = CounterVec::new(
        Opts::new("proxy_requests_total", "Total number of proxy requests"),
        &["method"],
    ).unwrap();

    let req_duration = Histogram::with_opts(
        HistogramOpts::new("proxy_request_duration_seconds", "Request duration in seconds")
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
    ).unwrap();

    let active_conns = Counter::new(
        "proxy_active_connections", "Current active connections",
    ).unwrap();

    registry.register(Box::new(req_counter.clone())).unwrap();
    registry.register(Box::new(req_duration.clone())).unwrap();
    registry.register(Box::new(active_conns.clone())).unwrap();

    REGISTRY.set(registry).ok();
    REQ_COUNTER.set(req_counter).ok();
    REQ_DURATION.set(req_duration).ok();
    ACTIVE_CONNS.set(active_conns).ok();
}

pub fn gather() -> String {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();
    let registry = REGISTRY.get().unwrap();
    let mut buffer = Vec::new();
    encoder.encode(&registry.gather(), &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}
