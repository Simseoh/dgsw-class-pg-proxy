-- ─────────────────────────────────────────────────────────────────────
-- authz.lua — Role 기반 인가 (Authorization)
-- auth.lua 이후 실행 — ctx.vars["role"] 을 참조
-- ─────────────────────────────────────────────────────────────────────

-- 경로 패턴 → 허용 role 목록
-- role이 없으면 인증된 모든 사용자 허용
local RULES = {
    -- Admin 전용
    { pattern = "^/admin/",   roles = {"admin"} },
    { pattern = "^/reload",   roles = {"admin"} },
    { pattern = "^/metrics",  roles = {"admin", "ops"} },

    -- 읽기 전용
    { pattern = "^/api/v1/",  roles = {"admin", "user", "reader"} },
    { pattern = "^/api/v2/",  roles = {"admin", "user"} },

    -- 공개 (인증 불필요 — auth.lua에서 이미 처리됨)
    { pattern = "^/health$",  roles = nil },
    { pattern = "^/public/",  roles = nil },
}

local function has_role(user_role, allowed_roles)
    if not allowed_roles then return true end   -- nil = 모든 role 허용
    for _, r in ipairs(allowed_roles) do
        if r == user_role then return true end
    end
    return false
end

local function find_rule(path)
    for _, rule in ipairs(RULES) do
        if path:match(rule.pattern) then
            return rule
        end
    end
    return nil  -- 규칙 없음 = 기본 허용
end

function on_request(ctx)
    local path      = ctx.req.path
    local user_role = ctx:get_var("role")
    local auth_meth = ctx:get_var("auth_method")

    -- 공개 경로는 스킵
    if auth_meth == "public" then return end

    local rule = find_rule(path)
    if not rule then
        -- 정의된 규칙 없음: 인증된 사용자면 허용
        if not user_role then
            ctx:reject(403, "Forbidden: authentication required")
        end
        return
    end

    if not has_role(user_role, rule.roles) then
        log.warn("Authz denied: path=" .. path
            .. " user_role=" .. (user_role or "none")
            .. " required=" .. (rule.roles and table.concat(rule.roles, "|") or "any"))
        ctx:reject(403, "Forbidden: insufficient privileges")
        return
    end

    log.info("Authz OK: path=" .. path .. " role=" .. (user_role or "none"))
end
