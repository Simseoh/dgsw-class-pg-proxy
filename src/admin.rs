use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::lua::PluginSources;
use crate::config::PluginsConfig;
use crate::tls::HotTlsAcceptor;
use crate::metrics;

pub struct AdminServer {
    addr:         String,
    plugin_srcs:  Arc<std::sync::RwLock<PluginSources>>,
    plugins_cfg:  PluginsConfig,
    tls_acceptor: Option<Arc<HotTlsAcceptor>>,
}

impl AdminServer {
    pub fn new(
        addr:         impl Into<String>,
        plugin_srcs:  Arc<std::sync::RwLock<PluginSources>>,
        plugins_cfg:  PluginsConfig,
        tls_acceptor: Option<Arc<HotTlsAcceptor>>,
    ) -> Self {
        Self {
            addr:         addr.into(),
            plugin_srcs,
            plugins_cfg,
            tls_acceptor,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(&self.addr).await?;
        info!("Admin API listening on http://{}", self.addr);

        let srcs = Arc::clone(&self.plugin_srcs);
        let cfg  = self.plugins_cfg.clone();
        let tls  = self.tls_acceptor.clone();

        loop {
            let (stream, peer) = listener.accept().await?;
            let srcs2 = Arc::clone(&srcs);
            let cfg2  = cfg.clone();
            let tls2  = tls.clone();

            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = service_fn(move |req: Request<Incoming>| {
                    let srcs3 = Arc::clone(&srcs2);
                    let cfg3  = cfg2.clone();
                    let tls3  = tls2.clone();
                    async move {
                        handle_admin(req, srcs3, cfg3, tls3).await
                    }
                });
                if let Err(e) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc)
                    .await
                {
                    warn!("Admin conn error from {}: {}", peer, e);
                }
            });
        }
    }
}

async fn handle_admin(
    req:         Request<Incoming>,
    srcs:        Arc<std::sync::RwLock<PluginSources>>,
    cfg:         PluginsConfig,
    tls:         Option<Arc<HotTlsAcceptor>>,
) -> anyhow::Result<Response<Full<Bytes>>> {
    let path   = req.uri().path().to_string();
    let method = req.method().clone();

    match (method.as_str(), path.as_str()) {
        // POST /reload — Lua 플러그인 핫 리로드
        ("POST", "/reload") => {
            match PluginSources::load(&cfg) {
                Ok(new_srcs) => {
                    *srcs.write().unwrap() = new_srcs;
                    info!("Plugins reloaded via Admin API");
                    ok_json(r#"{"status":"ok","message":"plugins reloaded"}"#)
                }
                Err(e) => {
                    warn!("Plugin reload failed: {}", e);
                    ok_json(&format!(r#"{{"status":"error","message":"{}"}}"#, e))
                }
            }
        }

        // POST /reload/tls — TLS 인증서 핫 로테이션
        ("POST", "/reload/tls") => {
            if let Some(acceptor) = tls {
                match acceptor.reload() {
                    Ok(_)  => ok_json(r#"{"status":"ok","message":"TLS reloaded"}"#),
                    Err(e) => ok_json(&format!(r#"{{"status":"error","message":"{}"}}"#, e)),
                }
            } else {
                ok_json(r#"{"status":"error","message":"TLS not enabled"}"#)
            }
        }

        // GET /metrics — Prometheus 메트릭
        ("GET", "/metrics") => {
            let body = metrics::gather();
            Ok(Response::builder()
                .status(200)
                .header("content-type", "text/plain; version=0.0.4")
                .body(Full::new(Bytes::from(body)))?)
        }

        // GET /health — Admin 헬스체크
        ("GET", "/health") => {
            ok_json(r#"{"status":"ok"}"#)
        }

        // GET /plugins — 현재 로드된 플러그인 목록
        ("GET", "/plugins") => {
            let guard = srcs.read().unwrap();
            let list: Vec<&str> = guard.pipeline.iter().map(|s| s.as_str()).collect();
            let body = format!(r#"{{"plugins":[{}]}}"#,
                list.iter().map(|s| format!(r#""{}""#, s)).collect::<Vec<_>>().join(","));
            ok_json(&body)
        }

        _ => {
            Ok(Response::builder()
                .status(404)
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(r#"{"error":"not found"}"#)))?)
        }
    }
}

fn ok_json(body: &str) -> anyhow::Result<Response<Full<Bytes>>> {
    Ok(Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.to_string())))?)
}
