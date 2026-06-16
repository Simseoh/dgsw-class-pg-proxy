-- ─────────────────────────────────────────────────────────────────────
-- transform.lua — 요청/응답 헤더 수정 및 변환
-- on_request:   업스트림으로 보내기 전 헤더 정제
-- on_route:     경로 기반 upstream 선택
-- on_response:  응답에 공통 헤더 추가
-- ─────────────────────────────────────────────────────────────────────

-- ── on_request: 요청 헤더 수정 ──────────────────────────────────────
function on_request(ctx)
    local user_id  = ctx:get_var("user_id")
    local user_role = ctx:get_var("role")

    -- 인증 정보를 업스트림 헤더로 전달
    if user_id then
        ctx:set_req_header("x-user-id",   user_id)
        ctx:set_req_header("x-user-role", user_role or "anonymous")
    end

    -- 민감한 헤더 제거 (upstream으로 전달하지 않음)
    ctx:set_req_header("authorization", "")   -- 업스트림에게 숨김
    ctx:set_req_header("x-api-key",     "")

    -- 요청 ID 생성 (트레이싱용)
    -- uuid 전역 함수가 Rust에서 등록되어 있으면 사용
    local req_id = (uuid and uuid.v4()) or tostring(os.time()) .. "-" .. tostring(math.random(100000))
    ctx:set_req_header("x-request-id", req_id)
    ctx:set_var("request_id", req_id)
end

-- ── on_route: upstream 선택 ──────────────────────────────────────────
function on_route(ctx)
    local path = ctx.req.path

    -- /api/v2/ 경로는 api_v2 upstream으로
    if path:match("^/api/v2/") then
        ctx:upstream("api_v2")
        return "api_v2"
    end

    -- 기본 upstream
    ctx:upstream("default")
    return "default"
end

-- ── on_response: 응답 헤더 수정 ─────────────────────────────────────
function on_response(ctx)
    -- 공통 보안 헤더 추가
    ctx:set_res_header("x-proxy",                "rust-lua-proxy/0.1")
    ctx:set_res_header("x-frame-options",        "DENY")
    ctx:set_res_header("x-content-type-options", "nosniff")
    ctx:set_res_header("referrer-policy",        "strict-origin-when-cross-origin")

    -- CORS (필요시 도메인 제한)
    local origin = ctx.req.headers["origin"]
    if origin then
        ctx:set_res_header("access-control-allow-origin", origin)
        ctx:set_res_header("vary", "Origin")
    end

    -- 요청 ID 에코
    local req_id = ctx:get_var("request_id")
    if req_id then
        ctx:set_res_header("x-request-id", req_id)
    end

    -- 업스트림이 Server 헤더를 노출하면 숨김
    ctx:set_res_header("server", "proxy")
end
