use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::UpstreamConfig;
use super::balancer::{make_balancer, Balancer, Target};

pub struct UpstreamPool {
    inner: Arc<RwLock<HashMap<String, Arc<Box<dyn Balancer>>>>>,
}

impl UpstreamPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn init(&self, configs: &[UpstreamConfig]) -> Result<()> {
        let mut map = self.inner.write().await;
        for cfg in configs {
            let targets: Vec<Target> = cfg.targets.iter()
                .map(|u| Target::new(u.clone()))
                .collect();
            let balancer = make_balancer(&cfg.lb, targets);
            info!("Upstream '{}' initialized ({} targets, lb={})",
                cfg.name, cfg.targets.len(), cfg.lb);
            map.insert(cfg.name.clone(), Arc::new(balancer));
        }
        Ok(())
    }

    /// upstream 이름으로 다음 타겟 URL 반환
    pub async fn next_url(&self, name: &str, key: Option<&str>) -> Option<String> {
        let map = self.inner.read().await;
        let lb = map.get(name).or_else(|| map.get("default"))?;
        lb.next(key)
    }

    /// 헬스체크 태스크 시작
    pub fn spawn_health_checks(&self, configs: Vec<UpstreamConfig>) {
        let pool = self.inner.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let map = pool.read().await;
                for cfg in &configs {
                    if let Some(lb) = map.get(&cfg.name) {
                        for target in lb.targets() {
                            let url = format!("{}{}", target.url, cfg.health_check_path);
                            let alive_ref = target.alive.clone();
                            tokio::spawn(async move {
                                let alive = reqwest_health_check(&url).await;
                                alive_ref.store(alive, std::sync::atomic::Ordering::Relaxed);
                            });
                        }
                    }
                }
            }
        });
    }
}

async fn reqwest_health_check(url: &str) -> bool {
    // hyper 클라이언트로 HEAD 요청
    let client = hyper_util::client::legacy::Client::builder(
        hyper_util::rt::TokioExecutor::new()
    ).build_http::<http_body_util::Empty<bytes::Bytes>>();

    match url.parse::<hyper::Uri>() {
        Ok(uri) => {
            let req = hyper::Request::builder()
                .method("HEAD")
                .uri(uri)
                .body(http_body_util::Empty::new())
                .unwrap();
            match client.request(req).await {
                Ok(resp) => {
                    let ok = resp.status().is_success() || resp.status().as_u16() == 404;
                    if !ok { warn!("Health check failed for {}: {}", url, resp.status()); }
                    ok
                }
                Err(e) => {
                    warn!("Health check error for {}: {}", url, e);
                    false
                }
            }
        }
        Err(_) => false,
    }
}
