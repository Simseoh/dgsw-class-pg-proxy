-- ─────────────────────────────────────────────────────────────────────
-- auth.lua — JWT / API Key 인증
-- on_request 훅: 인증 성공 시 user_id, role을 ctx.vars에 저장
-- ─────────────────────────────────────────────────────────────────────

-- 환경변수 또는 설정에서 시크릿을 읽는 것이 이상적
-- 여기서는 Rust 전역 ENV 테이블로 주입 (engine.rs setup_globals 참고)
local JWT_SECRET = (ENV and ENV.JWT_SECRET) or "my-super-secret-change-in-production"

-- API Key 목록 (key → role)
local API_KEYS = {
    ["test-api-key-admin"]  = "admin",
    ["test-api-key-reader"] = "reader",
}

-- ── 인증 불필요 경로 ────────────────────────────────────────────────
local PUBLIC_PATHS = {
    "^/health$",
    "^/metrics$",
    "^/public/",
}

local function is_public(path)
    for _, pattern in ipairs(PUBLIC_PATHS) do
        if path:match(pattern) then
            return true
        end
    end
    return false
end

-- ── JWT 로드 ────────────────────────────────────────────────────────
-- require 대신 직접 파일 읽기 (mlua의 require 경로 설정 필요)
local function load_jwt_lib()
    local f = io.open("plugins/lib/jwt.lua", "r")
    if not f then return nil, "jwt lib not found" end
    local src = f:read("*a")
    f:close()
    local chunk, err = load(src)
    if not chunk then return nil, err end
    return chunk()
end

local jwt_lib, jwt_err = load_jwt_lib()

-- ── API Key 인증 ────────────────────────────────────────────────────
local function try_api_key(ctx)
    local key = ctx.req.headers["x-api-key"]
        or ctx.req.headers["authorization"]:match("^ApiKey%s+(.+)") 
        or nil

    -- authorization 헤더 접근 시 nil 가능성 처리
    if not key then
        local auth = ctx.req.headers["authorization"]
        if auth then
            key = auth:match("^ApiKey%s+(.+)")
        end
    end

    if not key then return false end

    local role = API_KEYS[key]
    if role then
        ctx:set_var("user_id", "apikey:" .. key:sub(1, 8))
        ctx:set_var("role", role)
        ctx:set_var("auth_method", "api_key")
        log.info("API Key auth OK, role=" .. role)
        return true
    end
    return false
end

-- ── JWT 인증 ────────────────────────────────────────────────────────
local function try_jwt(ctx)
    local auth = ctx.req.headers["authorization"]
    if not auth then return false, "no authorization header" end
    if not auth:match("^[Bb]earer%s+") then return false, "not bearer token" end

    if not jwt_lib then
        log.warn("JWT lib unavailable: " .. (jwt_err or "unknown"))
        return false, "jwt lib unavailable"
    end

    local claims, err = jwt_lib.verify(auth, JWT_SECRET)
    if err then
        return false, err
    end

    ctx:set_var("user_id",    claims.sub or "unknown")
    ctx:set_var("role",       claims.role or "user")
    ctx:set_var("auth_method","jwt")
    log.info("JWT auth OK, sub=" .. (claims.sub or "?") .. " role=" .. (claims.role or "user"))
    return true
end

-- ── on_request 훅 ───────────────────────────────────────────────────
function on_request(ctx)
    -- 공개 경로는 인증 스킵
    if is_public(ctx.req.path) then
        ctx:set_var("auth_method", "public")
        return
    end

    -- 1) API Key 시도
    if try_api_key(ctx) then return end

    -- 2) JWT 시도
    local ok, err = try_jwt(ctx)
    if ok then return end

    -- 인증 실패
    log.warn("Auth failed: " .. (err or "no valid credential") .. " path=" .. ctx.req.path)
    ctx:set_res_header("www-authenticate", 'Bearer realm="proxy"')
    ctx:reject(401, "Unauthorized: " .. (err or "no valid credential"))
end
