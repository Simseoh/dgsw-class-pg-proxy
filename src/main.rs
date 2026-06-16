mod config;
mod handler;
mod lua;
mod metrics;
mod tls;
mod upstream;
mod admin;
mod watcher;

use std::cell::UnsafeCell;
use std::sync::Arc;

use anyhow::Result;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::task::LocalSet;
use tokio::runtime::Builder;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::lua::{LuaEngine, PluginSources};
use crate::upstream::UpstreamPool;
use crate::admin::AdminServer;

#[tokio::main]
async fn main() -> Result<()> {
    // rustls 0.23은 프로세스 기본 CryptoProvider를 명시적으로 정해주는 편이 안전함.
    // 현재 빌드에서는 ring/aws-lc-rs가 함께 활성화될 수 있어, 시작 시 ring을 고정한다.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // ── 로깅 초기화 ─────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // ── 설정 로드 ────────────────────────────────────────────────────
    let config_path = std::env::args().nth(1)
        .unwrap_or_else(|| "proxy.toml".to_string());

    let cfg = Config::load(&config_path)?;
    info!("Config loaded from '{}'", config_path);

    // ── Prometheus 메트릭 초기화 ─────────────────────────────────────
    metrics::init();

    // ── Lua 플러그인 소스 로드 ───────────────────────────────────────
    let plugin_srcs = Arc::new(std::sync::RwLock::new(
        PluginSources::load(&cfg.plugins)?,
    ));

    // ── 파일 감시 (핫 리로드) ────────────────────────────────────────
    watcher::spawn_watcher(cfg.plugins.clone(), Arc::clone(&plugin_srcs))?;

    // ── TLS Acceptor ─────────────────────────────────────────────────
    let tls_acceptor = if std::path::Path::new(&cfg.tls.cert).exists() {
        match tls::HotTlsAcceptor::new(&cfg.tls) {
            Ok(a) => {
                info!("TLS enabled (cert={})", cfg.tls.cert);
                Some(a)
            }
            Err(e) => {
                warn!("TLS init failed: {} — falling back to HTTP only", e);
                None
            }
        }
    } else {
        warn!("TLS cert not found ({}) — running HTTP only", cfg.tls.cert);
        None
    };

    // ── 업스트림 풀 ─────────────────────────────────────────────────
    let pool = Arc::new(UpstreamPool::new());
    pool.init(&cfg.upstream).await?;
    pool.spawn_health_checks(cfg.upstream.clone());

    // ── Admin API 서버 ───────────────────────────────────────────────
    let admin_srv = AdminServer::new(
        cfg.admin.addr.clone(),
        Arc::clone(&plugin_srcs),
        cfg.plugins.clone(),
        tls_acceptor.clone(),
    );
    tokio::spawn(async move {
        if let Err(e) = admin_srv.run().await {
            error!("Admin server error: {}", e);
        }
    });

    // ── HTTP 리스너 ──────────────────────────────────────────────────
    let http_addr = cfg.listener.http_addr.clone();
    let pool_http = Arc::clone(&pool);
    let srcs_http = Arc::clone(&plugin_srcs);
    let plugins_cfg_http = cfg.plugins.clone();

    tokio::spawn(async move {
        if let Err(e) = run_http_server(
            http_addr, pool_http, srcs_http, plugins_cfg_http,
        ).await {
            error!("HTTP server error: {}", e);
        }
    });

    // ── HTTPS 리스너 ─────────────────────────────────────────────────
    if let Some(acceptor) = tls_acceptor {
        let tls_addr = cfg.listener.addr.clone();
        let pool_tls = Arc::clone(&pool);
        let srcs_tls = Arc::clone(&plugin_srcs);
        let plugins_cfg_tls = cfg.plugins.clone();

        tokio::spawn(async move {
            if let Err(e) = run_https_server(
                tls_addr, acceptor, pool_tls, srcs_tls, plugins_cfg_tls,
            ).await {
                error!("HTTPS server error: {}", e);
            }
        });
    }

    // ── 그레이스풀 셧다운 ────────────────────────────────────────────
    info!("Proxy started. Press Ctrl+C to stop.");
    signal::ctrl_c().await?;
    info!("Shutting down gracefully...");

    Ok(())
}

/// HTTP 서버 루프
async fn run_http_server(
    addr:        String,
    pool:        Arc<UpstreamPool>,
    srcs:        Arc<std::sync::RwLock<PluginSources>>,
    _plugins_cfg: crate::config::PluginsConfig,
) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;
    info!("HTTP listening on http://{}", addr);

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let pool2 = Arc::clone(&pool);
        let srcs2 = Arc::clone(&srcs);

        // LocalSet으로 !Send Lua VM을 스레드에 고정
        tokio::task::spawn_blocking(move || {
            let rt = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build local runtime");
            let local = LocalSet::new();
            local.block_on(&rt, async move {
                // 요청별 Lua 엔진 (실제 프로덕션에서는 per-thread 캐싱)
                let engine = match LuaEngine::new(Arc::clone(&srcs2)) {
                    Ok(e)  => Arc::new(UnsafeCell::new(e)),
                    Err(e) => {
                        error!("LuaEngine init failed: {}", e);
                        return;
                    }
                };

                let io = TokioIo::new(stream);
                let svc = service_fn(move |req| {
                    let eng = Arc::clone(&engine);
                    let p   = Arc::clone(&pool2);
                    async move {
                        match handler::handle_request(req, peer_addr, eng, p).await {
                            Ok(r)  => Ok::<_, hyper::Error>(r),
                            Err(e) => {
                                error!("Handler error: {}", e);
                                Ok(handler::error_response(500, "Internal Server Error"))
                            }
                        }
                    }
                });

                if let Err(e) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc)
                    .await
                {
                    if !e.is_incomplete_message() {
                        warn!("HTTP conn error: {}", e);
                    }
                }
            });
        });
    }
}

/// HTTPS 서버 루프
async fn run_https_server(
    addr:        String,
    acceptor:    Arc<tls::HotTlsAcceptor>,
    pool:        Arc<UpstreamPool>,
    srcs:        Arc<std::sync::RwLock<PluginSources>>,
    _plugins_cfg: crate::config::PluginsConfig,
) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;
    info!("HTTPS listening on https://{}", addr);

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let pool2    = Arc::clone(&pool);
        let srcs2    = Arc::clone(&srcs);
        let acc      = acceptor.get();

        tokio::task::spawn_blocking(move || {
            let rt = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build local runtime");
            let local = LocalSet::new();
            local.block_on(&rt, async move {
                let tls_stream = match acc.accept(stream).await {
                    Ok(s)  => s,
                    Err(e) => { warn!("TLS handshake failed: {}", e); return; }
                };

                let engine = match LuaEngine::new(Arc::clone(&srcs2)) {
                    Ok(e)  => Arc::new(UnsafeCell::new(e)),
                    Err(e) => { error!("LuaEngine init: {}", e); return; }
                };

                let io = TokioIo::new(tls_stream);
                let svc = service_fn(move |req| {
                    let eng = Arc::clone(&engine);
                    let p   = Arc::clone(&pool2);
                    async move {
                        match handler::handle_request(req, peer_addr, eng, p).await {
                            Ok(r)  => Ok::<_, hyper::Error>(r),
                            Err(e) => {
                                error!("Handler error: {}", e);
                                Ok(handler::error_response(500, "Internal Server Error"))
                            }
                        }
                    }
                });

                if let Err(e) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc)
                    .await
                {
                    if !e.is_incomplete_message() {
                        warn!("HTTPS conn error: {}", e);
                    }
                }
            });
        });
    }
}
