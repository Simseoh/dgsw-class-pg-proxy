use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

#[derive(Debug, Clone)]
pub struct Target {
    pub url:    String,
    pub alive:  Arc<std::sync::atomic::AtomicBool>,
    pub active: Arc<AtomicUsize>,  // 현재 활성 연결 수 (least_conn용)
}

impl Target {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url:    url.into(),
            alive:  Arc::new(std::sync::atomic::AtomicBool::new(true)),
            active: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

pub trait Balancer: Send + Sync {
    /// 다음 업스트림 URL 선택
    fn next(&self, key: Option<&str>) -> Option<String>;
    fn targets(&self) -> &[Target];
}

// ── Round Robin ──────────────────────────────────────────────────────

pub struct RoundRobin {
    targets: Vec<Target>,
    counter: AtomicUsize,
}

impl RoundRobin {
    pub fn new(targets: Vec<Target>) -> Self {
        Self { targets, counter: AtomicUsize::new(0) }
    }
}

impl Balancer for RoundRobin {
    fn next(&self, _key: Option<&str>) -> Option<String> {
        let alive: Vec<&Target> = self.targets.iter().filter(|t| t.is_alive()).collect();
        if alive.is_empty() { return None; }
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % alive.len();
        Some(alive[idx].url.clone())
    }
    fn targets(&self) -> &[Target] { &self.targets }
}

// ── Least Connections ────────────────────────────────────────────────

pub struct LeastConn {
    targets: Vec<Target>,
}

impl LeastConn {
    pub fn new(targets: Vec<Target>) -> Self {
        Self { targets }
    }
}

impl Balancer for LeastConn {
    fn next(&self, _key: Option<&str>) -> Option<String> {
        self.targets.iter()
            .filter(|t| t.is_alive())
            .min_by_key(|t| t.active.load(Ordering::Relaxed))
            .map(|t| t.url.clone())
    }
    fn targets(&self) -> &[Target] { &self.targets }
}

// ── Consistent Hash ──────────────────────────────────────────────────

pub struct ConsistentHash {
    targets: Vec<Target>,
}

impl ConsistentHash {
    pub fn new(targets: Vec<Target>) -> Self {
        Self { targets }
    }

    fn hash_key(key: &str) -> u64 {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        h.finish()
    }
}

impl Balancer for ConsistentHash {
    fn next(&self, key: Option<&str>) -> Option<String> {
        let alive: Vec<&Target> = self.targets.iter().filter(|t| t.is_alive()).collect();
        if alive.is_empty() { return None; }
        let h = Self::hash_key(key.unwrap_or("default"));
        let idx = (h as usize) % alive.len();
        Some(alive[idx].url.clone())
    }
    fn targets(&self) -> &[Target] { &self.targets }
}

// ── 팩토리 ───────────────────────────────────────────────────────────

pub fn make_balancer(lb: &str, targets: Vec<Target>) -> Box<dyn Balancer> {
    match lb {
        "least_conn"       => Box::new(LeastConn::new(targets)),
        "consistent_hash"  => Box::new(ConsistentHash::new(targets)),
        _                  => Box::new(RoundRobin::new(targets)),
    }
}
