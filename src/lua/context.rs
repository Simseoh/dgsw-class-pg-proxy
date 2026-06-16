use mlua::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 요청 컨텍스트 — Lua 플러그인에 노출되는 공유 상태
#[derive(Debug, Clone)]
pub struct RequestCtx {
    // 요청
    pub req_method:  String,
    pub req_path:    String,
    pub req_headers: HashMap<String, String>,
    pub req_body:    String,
    pub client_ip:   String,

    // 응답 (on_response 단계에서 채워짐)
    pub res_status:  u16,
    pub res_headers: HashMap<String, String>,
    pub res_body:    String,

    // 플러그인 간 데이터 전달
    pub vars: HashMap<String, String>,

    // reject 결과 (status, message)
    pub rejected: Option<(u16, String)>,
}

impl RequestCtx {
    pub fn new(
        method: &str,
        path: &str,
        headers: HashMap<String, String>,
        body: String,
        client_ip: String,
    ) -> Self {
        Self {
            req_method:  method.to_string(),
            req_path:    path.to_string(),
            req_headers: headers,
            req_body:    body,
            client_ip,
            res_status:  200,
            res_headers: HashMap::new(),
            res_body:    String::new(),
            vars:        HashMap::new(),
            rejected:    None,
        }
    }

    pub fn is_rejected(&self) -> bool {
        self.rejected.is_some()
    }
}

// ── Lua UserData 래퍼 ────────────────────────────────────────────────

/// Lua에서 접근 가능한 ctx 객체
/// Arc<Mutex<>> 로 감싸 Lua 훅 여러 개가 같은 컨텍스트를 변경할 수 있게 함
#[derive(Clone)]
pub struct LuaCtx(pub Arc<Mutex<RequestCtx>>);

impl LuaUserData for LuaCtx {
    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        // ctx.req — 읽기 전용 서브테이블
        fields.add_field_method_get("req", |lua, this| {
            let ctx = this.0.lock().unwrap();
            let t = lua.create_table()?;

            t.set("method", ctx.req_method.clone())?;
            t.set("path",   ctx.req_path.clone())?;
            t.set("body",   ctx.req_body.clone())?;
            t.set("ip",     ctx.client_ip.clone())?;

            // headers 서브테이블
            let h = lua.create_table()?;
            for (k, v) in &ctx.req_headers {
                h.set(k.to_lowercase(), v.clone())?;
            }
            t.set("headers", h)?;
            Ok(t)
        });

        // ctx.res — 읽기/쓰기 서브테이블 (스냅샷 읽기, 쓰기는 메서드로)
        fields.add_field_method_get("res", |lua, this| {
            let ctx = this.0.lock().unwrap();
            let t = lua.create_table()?;
            t.set("status", ctx.res_status)?;
            t.set("body",   ctx.res_body.clone())?;

            let h = lua.create_table()?;
            for (k, v) in &ctx.res_headers {
                h.set(k.to_lowercase(), v.clone())?;
            }
            t.set("headers", h)?;
            Ok(t)
        });

        // ctx.vars — 읽기 (테이블 스냅샷)
        fields.add_field_method_get("vars", |lua, this| {
            let ctx = this.0.lock().unwrap();
            let t = lua.create_table()?;
            for (k, v) in &ctx.vars {
                t.set(k.clone(), v.clone())?;
            }
            Ok(t)
        });
    }

    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        // ctx:reject(status, message) — 즉시 거절
        methods.add_method("reject", |_, this, (status, msg): (u16, String)| {
            let mut ctx = this.0.lock().unwrap();
            ctx.rejected = Some((status, msg));
            Ok(())
        });

        // ctx:set_var(key, value) — ctx.vars 쓰기
        methods.add_method("set_var", |_, this, (k, v): (String, String)| {
            let mut ctx = this.0.lock().unwrap();
            ctx.vars.insert(k, v);
            Ok(())
        });

        // ctx:get_var(key) — ctx.vars 읽기
        methods.add_method("get_var", |_, this, k: String| {
            let ctx = this.0.lock().unwrap();
            Ok(ctx.vars.get(&k).cloned())
        });

        // ctx:set_req_header(key, value)
        methods.add_method("set_req_header", |_, this, (k, v): (String, String)| {
            let mut ctx = this.0.lock().unwrap();
            ctx.req_headers.insert(k.to_lowercase(), v);
            Ok(())
        });

        // ctx:set_res_header(key, value)
        methods.add_method("set_res_header", |_, this, (k, v): (String, String)| {
            let mut ctx = this.0.lock().unwrap();
            ctx.res_headers.insert(k.to_lowercase(), v);
            Ok(())
        });

        // ctx:set_res_status(status)
        methods.add_method("set_res_status", |_, this, status: u16| {
            let mut ctx = this.0.lock().unwrap();
            ctx.res_status = status;
            Ok(())
        });

        // ctx:get_req_header(key)
        methods.add_method("get_req_header", |_, this, k: String| {
            let ctx = this.0.lock().unwrap();
            Ok(ctx.req_headers.get(&k.to_lowercase()).cloned())
        });

        // ctx:upstream() — on_route에서 선택한 upstream 이름 반환
        methods.add_method("upstream", |_, this, name: String| {
            let mut ctx = this.0.lock().unwrap();
            ctx.vars.insert("__upstream__".into(), name);
            Ok(())
        });
    }
}
