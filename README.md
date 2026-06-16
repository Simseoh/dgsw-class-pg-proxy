# Rust + Lua Custom Proxy

보안 중심 고성능 프록시 서버 — Rust 코어 + Lua 플러그인 파이프라인

---

## 디렉토리 구조

```
proxy/
│
├── Cargo.toml                  # 크레이트 의존성 선언
├── proxy.toml                  # 런타임 설정 파일
├── README.md
│
├── certs/                      # TLS 인증서 (gitignore 권장)
│   ├── server.crt
│   └── server.key
│
├── src/                        # Rust 소스
│   ├── main.rs                 # 엔트리포인트 · 서버 루프 · 셧다운
│   ├── config.rs               # proxy.toml 파싱 (serde + toml)
│   ├── handler.rs              # HTTP 요청/응답 파이프라인
│   ├── tls.rs                  # rustls acceptor · 핫 인증서 로테이션
│   ├── metrics.rs              # Prometheus 메트릭 (Counter, Histogram)
│   ├── admin.rs                # Admin HTTP API 서버
│   ├── watcher.rs              # inotify 파일 감시 → 핫 리로드
│   │
│   ├── lua/
│   │   ├── mod.rs              # 모듈 재수출
│   │   ├── engine.rs           # Lua VM 관리 · 훅 실행 · 전역 함수 등록
│   │   └── context.rs          # ctx UserData (Rust ↔ Lua 인터페이스)
│   │
│   └── upstream/
│       ├── mod.rs
│       ├── balancer.rs         # Round-Robin / Least-Conn / Consistent Hash
│       └── pool.rs             # 커넥션 풀 · 헬스체크 태스크
│
└── plugins/                    # Lua 플러그인 (핫 리로드 대상)
    ├── ip_filter.lua           # ① IP allowlist / blocklist
    ├── rate_limit.lua          # ② 토큰 버킷 Rate Limiting
    ├── auth.lua                # ③ JWT / API Key 인증
    ├── authz.lua               # ④ Role 기반 인가
    ├── transform.lua           # ⑤ 헤더 변환 · upstream 라우팅
    └── lib/
        └── jwt.lua             # 순수 Lua JWT 라이브러리 (HS256)
```

---

## 빠른 시작

### 1. 의존성 설치

```bash
# Rust 설치 (https://rustup.rs)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### 2. 개발용 자체 서명 인증서 생성

```bash
mkdir -p certs
openssl req -x509 -newkey rsa:4096 \
  -keyout certs/server.key \
  -out    certs/server.crt \
  -days 365 -nodes \
  -subj "/CN=localhost"
```

### 3. 빌드 및 실행

```bash
# 개발 빌드
cargo build

# HTTP only (인증서 없이 테스트)
cargo run -- proxy.toml

# 릴리즈 빌드
cargo build --release
./target/release/proxy proxy.toml
```

### 4. 테스트

```bash
# 기본 요청 (HTTP)
curl http://localhost:8080/health

# API Key 인증
curl http://localhost:8080/api/v1/data \
  -H "x-api-key: test-api-key-admin"

# JWT 인증 (테스트 토큰 생성 필요)
curl http://localhost:8080/api/v1/data \
  -H "Authorization: Bearer <your-jwt-token>"

# 메트릭 확인
curl http://localhost:9000/metrics

# 플러그인 핫 리로드
curl -X POST http://localhost:9000/reload

# 플러그인 목록
curl http://localhost:9000/plugins
```

---

## 설정 파일 (`proxy.toml`)

```toml
[listener]
addr      = "0.0.0.0:8443"   # HTTPS
http_addr = "0.0.0.0:8080"   # HTTP

[tls]
cert = "certs/server.crt"
key  = "certs/server.key"

[admin]
addr = "127.0.0.1:9000"

[plugins]
dir      = "plugins"
pipeline = [
    "ip_filter.lua",
    "rate_limit.lua",
    "auth.lua",
    "authz.lua",
    "transform.lua",
]

[[upstream]]
name    = "default"
targets = ["http://127.0.0.1:3000", "http://127.0.0.1:3001"]
lb      = "round_robin"   # round_robin | least_conn | consistent_hash

[[upstream]]
name    = "api_v2"
targets = ["http://127.0.0.1:4000"]
lb      = "least_conn"
```

---

## Lua 플러그인 API

### ctx 객체

| 필드 / 메서드 | 타입 | 설명 |
|---|---|---|
| `ctx.req.method` | string | HTTP 메서드 |
| `ctx.req.path` | string | 요청 경로 |
| `ctx.req.headers["key"]` | string | 요청 헤더 읽기 |
| `ctx.req.body` | string | 요청 바디 |
| `ctx.req.ip` | string | 클라이언트 IP |
| `ctx.res.status` | number | 응답 상태 코드 |
| `ctx.res.headers["key"]` | string | 응답 헤더 읽기 |
| `ctx:get_req_header(k)` | string\|nil | 요청 헤더 읽기 (메서드) |
| `ctx:set_req_header(k, v)` | - | 요청 헤더 수정 |
| `ctx:set_res_header(k, v)` | - | 응답 헤더 수정 |
| `ctx:set_res_status(n)` | - | 응답 상태 코드 설정 |
| `ctx:get_var(k)` | string\|nil | 플러그인 간 변수 읽기 |
| `ctx:set_var(k, v)` | - | 플러그인 간 변수 저장 |
| `ctx:reject(status, msg)` | - | 즉시 거절 응답 반환 |
| `ctx:upstream(name)` | - | upstream 이름 선택 |

### Rust에서 제공하는 전역 함수

| 전역 | 설명 |
|---|---|
| `log.info(msg)` | 로그 출력 (INFO) |
| `log.warn(msg)` | 로그 출력 (WARN) |
| `base64.encode(s)` | Base64 인코딩 |
| `base64.decode(s)` | Base64 디코딩 |
| `hmac.sha256(msg, secret)` | HMAC-SHA256 raw bytes |
| `json.encode(v)` | Lua → JSON 문자열 |
| `uuid.v4()` | UUID v4 생성 |
| `ENV["KEY"]` | 환경변수 읽기 |

### 훅 함수

```lua
function on_request(ctx)   end  -- 요청 수신 직후 (IP필터→RateLimit→Auth→Authz)
function on_route(ctx)     end  -- upstream 선택 (string 반환)
function on_transform(ctx) end  -- 업스트림으로 보내기 전 헤더 변환
function on_response(ctx)  end  -- 응답 수신 후 (헤더 수정)
```

---

## Admin API

| 메서드 | 경로 | 설명 |
|---|---|---|
| `POST` | `/reload` | Lua 플러그인 핫 리로드 |
| `POST` | `/reload/tls` | TLS 인증서 무중단 로테이션 |
| `GET` | `/metrics` | Prometheus 메트릭 |
| `GET` | `/health` | Admin 헬스체크 |
| `GET` | `/plugins` | 로드된 플러그인 목록 |

---

## 플러그인 개발 가이드

새 플러그인 추가:

```lua
-- plugins/my_plugin.lua
function on_request(ctx)
    -- 요청 처리
    local header = ctx:get_req_header("x-custom-header")
    if not header then
        ctx:reject(400, "Missing x-custom-header")
        return
    end
    ctx:set_var("custom_value", header)
end

function on_response(ctx)
    -- 응답 처리
    ctx:set_res_header("x-custom-processed", "true")
end
```

`proxy.toml`의 `pipeline` 배열에 추가 후 `/reload` 호출:

```bash
curl -X POST http://localhost:9000/reload
```

---

## 환경변수

| 변수 | 설명 | 기본값 |
|---|---|---|
| `RUST_LOG` | 로그 레벨 | `info` |
| `JWT_SECRET` | JWT 서명 시크릿 (Lua ENV 테이블로 주입) | `my-super-secret-...` |

```bash
RUST_LOG=debug JWT_SECRET=my-production-secret ./target/release/proxy proxy.toml
```

---

## Phase별 구현 현황

| Phase | 내용 | 상태 |
|---|---|---|
| Phase 1 | TCP 리스너, HTTP 파싱, 업스트림 포워딩 | ✅ 완료 |
| Phase 2 | mlua 통합, ctx UserData, 훅 실행, 핫 리로드 | ✅ 완료 |
| Phase 3 | JWT/RateLimit/IP필터/Authz 플러그인 | ✅ 완료 |
| Phase 4 | rustls TLS, 로드밸런싱, Prometheus, 그레이스풀 셧다운 | ✅ 완료 |
| Phase 5 | Lua 샌드박스, WebSocket, mTLS, gRPC | 🔲 옵션 |
# dgsw-class-pg-proxy
