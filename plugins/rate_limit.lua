-- ─────────────────────────────────────────────────────────────────────
-- rate_limit.lua — 토큰 버킷 Rate Limiting
-- IP 또는 user_id별 독립 버킷
-- ─────────────────────────────────────────────────────────────────────

local CAPACITY   = 100   -- 최대 토큰 수
local REFILL_RATE = 10   -- 초당 보충 토큰 수

-- IP별 버킷 테이블 (Lua VM 생존 기간 동안 유지)
local buckets = {}

local function get_time()
    -- os.time()은 초 단위 — 충분한 정밀도
    return os.time()
end

local function get_bucket(key)
    if not buckets[key] then
        buckets[key] = {
            tokens = CAPACITY,
            last   = get_time(),
        }
    end
    return buckets[key]
end

local function refill(bucket, now)
    local elapsed = now - bucket.last
    if elapsed > 0 then
        bucket.tokens = math.min(CAPACITY, bucket.tokens + elapsed * REFILL_RATE)
        bucket.last   = now
    end
end

function on_request(ctx)
    -- 식별 키: user_id가 있으면 우선, 없으면 IP
    local key = ctx:get_var("user_id") or ctx.req.ip

    local now    = get_time()
    local bucket = get_bucket(key)
    refill(bucket, now)

    if bucket.tokens < 1 then
        -- 다음 보충까지 남은 시간
        local retry_after = math.ceil((1 - bucket.tokens) / REFILL_RATE)
        ctx:set_res_header("retry-after", tostring(retry_after))
        log.warn("Rate limit exceeded for " .. key)
        ctx:reject(429, "Too Many Requests")
        return
    end

    bucket.tokens = bucket.tokens - 1

    -- 남은 토큰 수를 헤더로 노출
    ctx:set_res_header("x-ratelimit-remaining", tostring(math.floor(bucket.tokens)))
    ctx:set_res_header("x-ratelimit-limit",     tostring(CAPACITY))
end
