use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::net::SocketAddr;

use anyhow::Result;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response};
use hyper::body::Incoming;
use tracing::{info, warn, error};

use crate::lua::{LuaEngine, RequestCtx};
use crate::upstream::UpstreamPool;
use crate::metrics;

/// 단일 HTTP 요청 처리
pub async fn handle_request(
    req:        Request<Incoming>,
    client_ip:  SocketAddr,
    engine:     Arc<std::cell::UnsafeCell<LuaEngine>>, // per-thread, LocalSet
    pool:       Arc<UpstreamPool>,
) -> Result<Response<Full<Bytes>>> {
    metrics::REQ_COUNTER
        .get()
        .unwrap()
        .with_label_values(&[req.method().as_str()])
        .inc();
    let timer = metrics::REQ_DURATION.get().unwrap().start_timer();

    // ── 1. 요청 파싱 ────────────────────────────────────────────────
    let method = req.method().to_string();
    let path   = req.uri().path().to_string();

    let mut headers: HashMap<String, String> = HashMap::new();
    for (k, v) in req.headers() {
        if let Ok(val) = v.to_str() {
            headers.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }

    let ip = match headers.get("x-real-ip")
        .or_else(|| headers.get("x-forwarded-for")) {
        Some(h) => h.clone(),
        None    => client_ip.ip().to_string(),
    };

    let body_bytes = req.collect().await?.to_bytes();
    let body_str   = String::from_utf8_lossy(&body_bytes).to_string();

    // ── 2. RequestCtx 생성 ──────────────────────────────────────────
    let ctx = Arc::new(Mutex::new(RequestCtx::new(
        &method, &path, headers.clone(), body_str.clone(), ip.clone(),
    )));

    // ── 3. Lua on_request 파이프라인 ────────────────────────────────
    // Safety: LuaEngine은 LocalSet 내 단일 스레드에서만 접근
    let eng = unsafe { &*engine.get() };
    if let Err(e) = eng.run_on_request(ctx.clone()) {
        error!("on_request error: {}", e);
    }

    // reject 됐으면 즉시 응답
    if let Some((status, msg)) = ctx.lock().unwrap().rejected.clone() {
        timer.observe_duration();
        metrics::REQ_COUNTER
            .get()
            .unwrap()
            .with_label_values(&["rejected"])
            .inc();
        return Ok(error_response(status, &msg));
    }

    // ── 4. on_route — upstream 선택 ─────────────────────────────────
    let upstream_name = match eng.run_on_route(ctx.clone()) {
        Ok(Some(n)) => n,
        _           => "default".to_string(),
    };

    // ── 5. on_transform — 헤더 수정 ─────────────────────────────────
    let _ = eng.run_on_transform(ctx.clone());

    // ── 6. 업스트림 포워딩 ──────────────────────────────────────────
    let key = ip.as_str();
    let target_url = match pool.next_url(&upstream_name, Some(key)).await {
        Some(u) => u,
        None => {
            warn!("No healthy upstream for '{}'", upstream_name);
            timer.observe_duration();
            return Ok(error_response(503, "Service Unavailable"));
        }
    };

    let forward_url = format!("{}{}", target_url.trim_end_matches('/'), path);

    // 수정된 헤더 반영
    let ctx_snap = ctx.lock().unwrap();
    let mut fwd_headers = ctx_snap.req_headers.clone();
    fwd_headers.insert("x-forwarded-for".into(), ip.clone());
    fwd_headers.insert("x-proxy".into(), "rust-lua-proxy/0.1".into());
    drop(ctx_snap);

    let upstream_resp = forward_request(
        &method, &forward_url, fwd_headers, body_bytes,
    ).await;

    let (res_status, res_headers, res_body) = match upstream_resp {
        Ok(r) => r,
        Err(e) => {
            error!("Upstream error: {}", e);
            timer.observe_duration();
            return Ok(error_response(502, "Bad Gateway"));
        }
    };

    // ── 7. on_response ──────────────────────────────────────────────
    {
        let mut ctx_w = ctx.lock().unwrap();
        ctx_w.res_status  = res_status;
        ctx_w.res_headers = res_headers.clone();
        ctx_w.res_body    = String::from_utf8_lossy(&res_body).to_string();
    }
    let _ = eng.run_on_response(ctx.clone());

    // ── 8. 최종 응답 조립 ───────────────────────────────────────────
    let ctx_final = ctx.lock().unwrap();
    let mut builder = Response::builder()
        .status(ctx_final.res_status);

    for (k, v) in &ctx_final.res_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    // 응답 바디: on_response가 수정했으면 수정본, 아니면 원본
    let final_body = if ctx_final.res_body.is_empty() {
        Bytes::from(res_body)
    } else {
        Bytes::from(ctx_final.res_body.clone())
    };

    timer.observe_duration();
    info!("{} {} -> {} ({}) upstream={}", method, path, ctx_final.res_status, ip, upstream_name);

    Ok(builder.body(Full::new(final_body))?)
}

/// 업스트림으로 요청 포워딩
async fn forward_request(
    method:  &str,
    url:     &str,
    headers: HashMap<String, String>,
    body:    Bytes,
) -> Result<(u16, HashMap<String, String>, Vec<u8>)> {
    let client = hyper_util::client::legacy::Client::builder(
        hyper_util::rt::TokioExecutor::new()
    ).build_http::<Full<Bytes>>();

    let uri: hyper::Uri = url.parse()?;
    let mut req_builder = Request::builder()
        .method(method)
        .uri(uri);

    for (k, v) in &headers {
        req_builder = req_builder.header(k.as_str(), v.as_str());
    }
    let req = req_builder.body(Full::new(body))?;

    let resp = client.request(req).await?;
    let status = resp.status().as_u16();

    let mut res_headers = HashMap::new();
    for (k, v) in resp.headers() {
        if let Ok(val) = v.to_str() {
            res_headers.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }

    let body_bytes = resp.collect().await?.to_bytes().to_vec();
    Ok((status, res_headers, body_bytes))
}

/// 에러 응답 생성
pub fn error_response(status: u16, msg: &str) -> Response<Full<Bytes>> {
    let body = format!(r#"{{"error":"{}","status":{}}}"#, msg, status);
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}
