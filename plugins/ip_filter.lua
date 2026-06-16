-- ─────────────────────────────────────────────────────────────────────
-- ip_filter.lua — IP allowlist / blocklist 필터
-- 플러그인 파이프라인 1순위: 가장 빠른 거절
-- ─────────────────────────────────────────────────────────────────────

-- 설정: 직접 편집하거나 향후 외부 파일로 분리 가능
local BLOCKLIST = {
    -- "192.168.1.100",
    -- "10.0.0.0/8",  -- CIDR (Rust 측 ctx:get_var("ip")로 전달된 IP와 매칭)
}

local ALLOWLIST = {
    -- 비어있으면 모든 IP 허용
    -- "127.0.0.1",
    -- "10.0.0.0/8",
}

-- 단순 IP 문자열 매칭 (CIDR 지원은 Rust libs에 위임)
local function ip_matches(ip, list)
    for _, pattern in ipairs(list) do
        if ip == pattern then
            return true
        end
        -- 단순 prefix 매칭 (예: "10.0." 로 시작하는 IP)
        if pattern:sub(-1) == "*" then
            local prefix = pattern:sub(1, -2)
            if ip:sub(1, #prefix) == prefix then
                return true
            end
        end
    end
    return false
end

function on_request(ctx)
    local ip = ctx.req.ip

    -- blocklist 체크
    if #BLOCKLIST > 0 and ip_matches(ip, BLOCKLIST) then
        log.warn("IP blocked: " .. ip)
        ctx:reject(403, "Forbidden: your IP is blocked")
        return
    end

    -- allowlist 체크 (비어있으면 스킵)
    if #ALLOWLIST > 0 and not ip_matches(ip, ALLOWLIST) then
        log.warn("IP not in allowlist: " .. ip)
        ctx:reject(403, "Forbidden: IP not allowed")
        return
    end
end
