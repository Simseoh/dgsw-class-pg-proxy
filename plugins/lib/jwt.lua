-- ─────────────────────────────────────────────────────────────────────
-- lib/jwt.lua — 순수 Lua JWT 검증 라이브러리 (HS256)
-- Rust의 base64 / hmac 전역 함수를 활용
-- ─────────────────────────────────────────────────────────────────────

local jwt = {}

-- ── Base64url 인코딩/디코딩 ─────────────────────────────────────────

local function b64url_decode(s)
    -- base64url → base64 변환
    s = s:gsub("-", "+"):gsub("_", "/")
    -- 패딩 추가
    local pad = (4 - #s % 4) % 4
    s = s .. string.rep("=", pad)
    return base64.decode(s) or ""
end

local function b64url_encode(s)
    local encoded = base64.encode(s)
    -- base64 → base64url 변환
    encoded = encoded:gsub("+", "-"):gsub("/", "_"):gsub("=+$", "")
    return encoded
end

-- ── JSON 간이 파서 ──────────────────────────────────────────────────

local function parse_json_obj(s)
    local result = {}
    -- 문자열 값 파싱: "key":"value"
    for key, val in s:gmatch('"([^"]+)"%s*:%s*"([^"]*)"') do
        result[key] = val
    end
    -- 숫자/불린 값 파싱: "key":value
    for key, val in s:gmatch('"([^"]+)"%s*:%s*([%d%.%-]+)') do
        result[key] = tonumber(val)
    end
    -- boolean
    for key, val in s:gmatch('"([^"]+)"%s*:%s*(true|false)') do
        result[key] = (val == "true")
    end
    return result
end

-- ── HMAC-SHA256 (Rust 전역 함수 위임) ──────────────────────────────
-- Rust의 setup_globals()에서 hmac_sha256(msg, key) → hex string 을 등록해야 함
-- 여기서는 Rust 측 전역으로 제공되는 hmac.sha256 함수를 사용

local function hmac_sha256_b64url(msg, secret)
    -- Rust 전역 hmac.sha256(msg, secret) → raw bytes (string)
    if hmac and hmac.sha256 then
        local raw = hmac.sha256(msg, secret)
        return b64url_encode(raw)
    end
    -- fallback: hmac 전역 없으면 검증 불가
    return nil
end

-- ── 공개 API ────────────────────────────────────────────────────────

--- JWT 검증
--- @param token  string   "Bearer eyJ..." 형식 또는 raw token
--- @param secret string   HMAC-SHA256 시크릿 키
--- @return table|nil, string|nil  claims 또는 nil + 에러 메시지
function jwt.verify(token, secret)
    -- "Bearer " 접두사 제거
    local raw = token:match("^[Bb]earer%s+(.+)") or token
    raw = raw:match("^%s*(.-)%s*$")  -- trim

    -- 3부분으로 분리
    local header_b64, payload_b64, sig_b64 = raw:match("^([^.]+)%.([^.]+)%.([^.]+)$")
    if not header_b64 then
        return nil, "invalid JWT format"
    end

    -- 헤더 파싱
    local header_json = b64url_decode(header_b64)
    local header = parse_json_obj(header_json)
    if header.alg ~= "HS256" then
        return nil, "unsupported algorithm: " .. (header.alg or "none")
    end

    -- 서명 검증
    local signing_input = header_b64 .. "." .. payload_b64
    local expected_sig  = hmac_sha256_b64url(signing_input, secret)
    if not expected_sig then
        return nil, "hmac.sha256 not available"
    end

    -- 시그니처 비교 (timing-safe는 Rust에서, 여기선 간단 비교)
    if expected_sig ~= sig_b64 then
        return nil, "signature verification failed"
    end

    -- 페이로드 파싱
    local payload_json = b64url_decode(payload_b64)
    local claims = parse_json_obj(payload_json)

    -- 만료 시간 체크
    if claims.exp then
        local now = os.time()
        if now > claims.exp then
            return nil, "token expired"
        end
    end

    -- nbf (not before) 체크
    if claims.nbf then
        local now = os.time()
        if now < claims.nbf then
            return nil, "token not yet valid"
        end
    end

    return claims, nil
end

--- JWT 생성 (테스트용)
--- @param claims table
--- @param secret string
--- @return string
function jwt.sign(claims, secret)
    local header  = b64url_encode('{"alg":"HS256","typ":"JWT"}')
    local payload = b64url_encode(json.encode(claims))
    local signing_input = header .. "." .. payload
    local sig = hmac_sha256_b64url(signing_input, secret)
    return signing_input .. "." .. sig
end

return jwt
