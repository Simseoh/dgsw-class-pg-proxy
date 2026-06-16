use mlua::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use anyhow::Result as AnyResult;
use tracing::{info, warn, error};

use crate::config::PluginsConfig;
use super::context::{LuaCtx, RequestCtx};

/// 로드된 플러그인 (이름 → Lua 소스코드)
#[derive(Debug, Clone, Default)]
pub struct PluginSources {
    pub pipeline: Vec<String>,              // 순서 있는 플러그인 이름
    pub sources:  HashMap<String, String>,  // 이름 → 소스코드
}

impl PluginSources {
    pub fn load(cfg: &PluginsConfig) -> AnyResult<Self> {
        let mut sources = HashMap::new();
        for name in &cfg.pipeline {
            let path = PathBuf::from(&cfg.dir).join(name);
            match std::fs::read_to_string(&path) {
                Ok(src) => {
                    info!("Loaded plugin: {}", name);
                    sources.insert(name.clone(), src);
                }
                Err(e) => {
                    warn!("Failed to load plugin {}: {}", name, e);
                }
            }
        }
        Ok(Self {
            pipeline: cfg.pipeline.clone(),
            sources,
        })
    }
}

/// Lua 엔진 — 스레드당 1개 VM 유지
/// Arc<RwLock<PluginSources>> 로 핫 리로드 시 소스만 교체
pub struct LuaEngine {
    lua:     Lua,
    sources: Arc<RwLock<PluginSources>>,
}

impl LuaEngine {
    pub fn new(sources: Arc<RwLock<PluginSources>>) -> LuaResult<Self> {
        let lua = Lua::new();
        let engine = Self { lua, sources };
        engine.setup_globals()?;
        Ok(engine)
    }

    /// 전역 유틸 함수 등록
    fn setup_globals(&self) -> LuaResult<()> {
        let globals = self.lua.globals();

        // json.encode / json.decode (간단 구현)
        let json_tbl = self.lua.create_table()?;
        json_tbl.set("encode", self.lua.create_function(|_, v: LuaValue| {
            Ok(lua_value_to_json(&v))
        })?)?;
        globals.set("json", json_tbl)?;

        // log.info / log.warn
        let log_tbl = self.lua.create_table()?;
        log_tbl.set("info", self.lua.create_function(|_, msg: String| {
            info!("[Lua] {}", msg);
            Ok(())
        })?)?;
        log_tbl.set("warn", self.lua.create_function(|_, msg: String| {
            warn!("[Lua] {}", msg);
            Ok(())
        })?)?;
        globals.set("log", log_tbl)?;

        // base64.encode / base64.decode
        let b64 = self.lua.create_table()?;
        b64.set("encode", self.lua.create_function(|_, s: String| {
            Ok(base64_encode(s.as_bytes()))
        })?)?;
        b64.set("decode", self.lua.create_function(|_, s: String| {
            Ok(base64_decode(&s).unwrap_or_default())
        })?)?;
        self.lua.globals().set("base64", b64)?;

        // hmac.sha256(msg, secret) → raw bytes string
        let hmac_tbl = self.lua.create_table()?;
        hmac_tbl.set("sha256", self.lua.create_function(|_, (msg, secret): (String, String)| {
            Ok(hmac_sha256_raw(msg.as_bytes(), secret.as_bytes()))
        })?)?;
        self.lua.globals().set("hmac", hmac_tbl)?;

        // uuid.v4() → string
        let uuid_tbl = self.lua.create_table()?;
        uuid_tbl.set("v4", self.lua.create_function(|_, ()| {
            Ok(uuid_v4())
        })?)?;
        self.lua.globals().set("uuid", uuid_tbl)?;

        // ENV 테이블 — 환경변수 주입
        let env_tbl = self.lua.create_table()?;
        for (k, v) in std::env::vars() {
            env_tbl.set(k, v)?;
        }
        self.lua.globals().set("ENV", env_tbl)?;

        Ok(())
    }

    /// 플러그인 로드 (chunk 컴파일)
    fn load_plugin(&self, name: &str, src: &str) -> LuaResult<()> {
        let chunk = self.lua.load(src).set_name(name);
        chunk.exec()?;
        Ok(())
    }

    /// on_request 훅 실행
    /// 반환: rejected면 Some((status, msg))
    pub fn run_on_request(&self, ctx: Arc<std::sync::Mutex<RequestCtx>>) -> LuaResult<()> {
        let srcs = self.sources.read().unwrap();
        for name in &srcs.pipeline {
            if let Some(src) = srcs.sources.get(name) {
                // 플러그인 로드 (매 요청마다 fresh 환경 — 단순 구현)
                if let Err(e) = self.load_plugin(name, src) {
                    error!("Plugin load error {}: {}", name, e);
                    continue;
                }
                let globals = self.lua.globals();
                if let Ok(func) = globals.get::<LuaFunction>("on_request") {
                    let lua_ctx = LuaCtx(ctx.clone());
                    if let Err(e) = func.call::<()>(lua_ctx) {
                        error!("on_request error in {}: {}", name, e);
                    }
                    // reject 됐으면 파이프라인 중단
                    if ctx.lock().unwrap().is_rejected() {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// on_route 훅 실행 — upstream 이름 반환
    pub fn run_on_route(&self, ctx: Arc<std::sync::Mutex<RequestCtx>>) -> LuaResult<Option<String>> {
        let srcs = self.sources.read().unwrap();
        for name in &srcs.pipeline {
            if let Some(src) = srcs.sources.get(name) {
                if let Err(e) = self.load_plugin(name, src) {
                    error!("Plugin load error {}: {}", name, e);
                    continue;
                }
                let globals = self.lua.globals();
                if let Ok(func) = globals.get::<LuaFunction>("on_route") {
                    let lua_ctx = LuaCtx(ctx.clone());
                    if let Ok(upstream) = func.call::<Option<String>>(lua_ctx) {
                        if upstream.is_some() {
                            return Ok(upstream);
                        }
                    }
                }
            }
        }
        // ctx.vars["__upstream__"] 확인
        let up = ctx.lock().unwrap().vars.get("__upstream__").cloned();
        Ok(up)
    }

    /// on_transform 훅 실행
    pub fn run_on_transform(&self, ctx: Arc<std::sync::Mutex<RequestCtx>>) -> LuaResult<()> {
        let srcs = self.sources.read().unwrap();
        for name in &srcs.pipeline {
            if let Some(src) = srcs.sources.get(name) {
                if let Err(e) = self.load_plugin(name, src) {
                    error!("Plugin load error {}: {}", name, e);
                    continue;
                }
                let globals = self.lua.globals();
                if let Ok(func) = globals.get::<LuaFunction>("on_transform") {
                    let lua_ctx = LuaCtx(ctx.clone());
                    if let Err(e) = func.call::<()>(lua_ctx) {
                        error!("on_transform error in {}: {}", name, e);
                    }
                }
            }
        }
        Ok(())
    }

    /// on_response 훅 실행
    pub fn run_on_response(&self, ctx: Arc<std::sync::Mutex<RequestCtx>>) -> LuaResult<()> {
        let srcs = self.sources.read().unwrap();
        for name in &srcs.pipeline {
            if let Some(src) = srcs.sources.get(name) {
                if let Err(e) = self.load_plugin(name, src) {
                    error!("Plugin load error {}: {}", name, e);
                    continue;
                }
                let globals = self.lua.globals();
                if let Ok(func) = globals.get::<LuaFunction>("on_response") {
                    let lua_ctx = LuaCtx(ctx.clone());
                    if let Err(e) = func.call::<()>(lua_ctx) {
                        error!("on_response error in {}: {}", name, e);
                    }
                }
            }
        }
        Ok(())
    }
}

// ── 유틸 함수 ─────────────────────────────────────────────────────────

fn lua_value_to_json(v: &LuaValue) -> String {
    match v {
        LuaValue::String(s)  => match s.to_str() {
            Ok(text) => format!("\"{}\"", text.replace('"', "\\\"")),
            Err(_) => "\"\"".into(),
        },
        LuaValue::Integer(i) => i.to_string(),
        LuaValue::Number(n)  => n.to_string(),
        LuaValue::Boolean(b) => b.to_string(),
        LuaValue::Nil        => "null".into(),
        LuaValue::Table(t)   => {
            // 배열인지 맵인지 판단
            let len = t.clone().raw_len();
            if len > 0 {
                let items: Vec<String> = (1..=len)
                    .filter_map(|i| t.get::<LuaValue>(i).ok())
                    .map(|v| lua_value_to_json(&v))
                    .collect();
                format!("[{}]", items.join(","))
            } else {
                let mut pairs = vec![];
                for pair in t.clone().pairs::<LuaValue, LuaValue>() {
                    if let Ok((k, v)) = pair {
                        let key = lua_value_to_json(&k);
                        let val = lua_value_to_json(&v);
                        pairs.push(format!("{}:{}", key, val));
                    }
                }
                format!("{{{}}}", pairs.join(","))
            }
        }
        _ => "null".into(),
    }
}

fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = match chunk.len() {
            1 => [chunk[0], 0, 0],
            2 => [chunk[0], chunk[1], 0],
            _ => [chunk[0], chunk[1], chunk[2]],
        };
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let _ = write!(out, "{}{}{}{}",
            CHARS[((n >> 18) & 63) as usize] as char,
            CHARS[((n >> 12) & 63) as usize] as char,
            if chunk.len() > 1 { CHARS[((n >> 6) & 63) as usize] as char } else { '=' },
            if chunk.len() > 2 { CHARS[(n & 63) as usize] as char } else { '=' },
        );
    }
    out
}

fn base64_decode(s: &str) -> Option<String> {
    // 간단 구현 — stdlib 없이
    const DECODE: [i8; 128] = {
        let mut t = [-1i8; 128];
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0usize;
        while i < chars.len() { t[chars[i] as usize] = i as i8; i += 1; }
        t
    };
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < bytes.len() {
        let c = [bytes[i], bytes[i+1], bytes[i+2], bytes[i+3]];
        if c.iter().any(|&b| b == b'=') { break; }
        let v: Vec<i8> = c.iter().map(|&b| if b < 128 { DECODE[b as usize] } else { -1 }).collect();
        if v.iter().any(|&x| x < 0) { return None; }
        let n = ((v[0] as u32) << 18) | ((v[1] as u32) << 12) | ((v[2] as u32) << 6) | v[3] as u32;
        out.push(((n >> 16) & 0xFF) as u8);
        out.push(((n >> 8)  & 0xFF) as u8);
        out.push((n & 0xFF) as u8);
        i += 4;
    }
    String::from_utf8(out).ok()
}

/// HMAC-SHA256: msg와 secret로 raw bytes 반환 (Lua에서 base64url 변환)
fn hmac_sha256_raw(msg: &[u8], secret: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret)
        .expect("HMAC can take key of any size");
    mac.update(msg);
    let result = mac.finalize().into_bytes();
    // raw bytes를 Latin-1 문자열로 (Lua에서 base64 인코딩)
    result.iter().map(|&b| b as char).collect()
}

/// UUID v4 간단 생성
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    // 의사 난수 (프로덕션에서는 uuid crate 사용 권장)
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        ns,
        (ns >> 16) & 0xFFFF,
        (ns >> 4) & 0x0FFF,
        0x8000 | ((ns >> 2) & 0x3FFF),
        (ns as u64).wrapping_mul(6364136223846793005)
    )
}
