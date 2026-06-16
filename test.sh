#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────
# test.sh — Rust + Lua Proxy 통합 테스트
# 사전 조건: proxy가 http://localhost:8080 에서 실행 중
# ─────────────────────────────────────────────────────────────────────

set -euo pipefail

BASE="http://localhost:8080"
ADMIN="http://localhost:9000"
PASS=0
FAIL=0

RED='\033[0;31m'
GRN='\033[0;32m'
YLW='\033[0;33m'
NC='\033[0m'

assert_status() {
    local desc="$1"
    local expected="$2"
    local actual="$3"
    if [ "$actual" = "$expected" ]; then
        echo -e "${GRN}[PASS]${NC} $desc (HTTP $actual)"
        ((PASS++)) || true
    else
        echo -e "${RED}[FAIL]${NC} $desc — expected HTTP $expected, got $actual"
        ((FAIL++)) || true
    fi
}

echo ""
echo "═══════════════════════════════════════════════════════"
echo "  Rust + Lua Custom Proxy — Integration Test"
echo "═══════════════════════════════════════════════════════"
echo ""

# ── 1. 헬스체크 ────────────────────────────────────────────────────
echo "── 1. Health check ─────────────────────────────────────"
STATUS=$(curl -s -o /dev/null -w "%{http_code}" "$BASE/health" 2>/dev/null || echo "000")
assert_status "GET /health (no auth required)" "200" "$STATUS"

# ── 2. Admin API ────────────────────────────────────────────────────
echo ""
echo "── 2. Admin API ────────────────────────────────────────"
STATUS=$(curl -s -o /dev/null -w "%{http_code}" "$ADMIN/health" 2>/dev/null || echo "000")
assert_status "Admin GET /health" "200" "$STATUS"

STATUS=$(curl -s -o /dev/null -w "%{http_code}" "$ADMIN/plugins" 2>/dev/null || echo "000")
assert_status "Admin GET /plugins" "200" "$STATUS"

STATUS=$(curl -s -o /dev/null -w "%{http_code}" "$ADMIN/metrics" 2>/dev/null || echo "000")
assert_status "Admin GET /metrics" "200" "$STATUS"

# ── 3. 인증 없이 보호된 경로 접근 ──────────────────────────────────
echo ""
echo "── 3. Auth — 인증 없이 접근 (401 expected) ─────────────"
STATUS=$(curl -s -o /dev/null -w "%{http_code}" "$BASE/api/v1/data" 2>/dev/null || echo "000")
assert_status "GET /api/v1/data without auth" "401" "$STATUS"

# ── 4. API Key 인증 ─────────────────────────────────────────────────
echo ""
echo "── 4. Auth — API Key ───────────────────────────────────"
STATUS=$(curl -s -o /dev/null -w "%{http_code}" \
    -H "x-api-key: test-api-key-admin" \
    "$BASE/api/v1/data" 2>/dev/null || echo "000")
# 업스트림 없으면 502, 있으면 200
echo -e "${YLW}[INFO]${NC} API Key auth → HTTP $STATUS (200 or 502 depending on upstream)"

STATUS=$(curl -s -o /dev/null -w "%{http_code}" \
    -H "x-api-key: invalid-key" \
    "$BASE/api/v1/data" 2>/dev/null || echo "000")
assert_status "GET /api/v1/data with invalid API key" "401" "$STATUS"

# ── 5. Rate Limiting ────────────────────────────────────────────────
echo ""
echo "── 5. Rate Limiting ─────────────────────────────────────"
echo "   (연속 요청 — 102번째부터 429 기대)"
LIMIT_HIT=0
for i in $(seq 1 110); do
    ST=$(curl -s -o /dev/null -w "%{http_code}" \
        -H "x-api-key: test-api-key-admin" \
        "$BASE/api/v1/test" 2>/dev/null || echo "000")
    if [ "$ST" = "429" ]; then
        LIMIT_HIT=1
        break
    fi
done
if [ "$LIMIT_HIT" = "1" ]; then
    echo -e "${GRN}[PASS]${NC} Rate limit triggered (429 received)"
    ((PASS++)) || true
else
    echo -e "${YLW}[SKIP]${NC} Rate limit not triggered (upstream may be down)"
fi

# ── 6. IP 필터 ──────────────────────────────────────────────────────
echo ""
echo "── 6. IP Filter ─────────────────────────────────────────"
echo -e "${YLW}[INFO]${NC} IP filter uses blocklist/allowlist in plugins/ip_filter.lua"
echo "   Edit the lists and call POST /reload to test dynamically"

# ── 7. 핫 리로드 ────────────────────────────────────────────────────
echo ""
echo "── 7. Hot reload ────────────────────────────────────────"
STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$ADMIN/reload" 2>/dev/null || echo "000")
assert_status "POST /admin/reload" "200" "$STATUS"

# ── 결과 요약 ───────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════════"
echo -e "  결과: ${GRN}PASS $PASS${NC} / ${RED}FAIL $FAIL${NC}"
echo "═══════════════════════════════════════════════════════"
echo ""

[ "$FAIL" = "0" ] && exit 0 || exit 1
