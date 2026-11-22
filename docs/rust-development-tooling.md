# Rust 개발 도구 및 의존성 기준

이 문서는 HTTP 네트워크 인스펙터를 Rust만으로 개발할 때 사용하는 도구와 크레이트를 정의한다. 버전 정보는 2026-11-17 기준이다.

JavaScript, TypeScript, Node.js는 사용하지 않는다. 데스크톱 UI는 `egui`와 `eframe`으로 구현한다.

## 기본 툴체인

프로젝트는 다음 도구를 사용한다.

| 도구 | 역할 | 구분 |
|---|---|---|
| `rustup` | Rust 툴체인 설치와 버전 관리 | 필수 |
| `cargo` | 의존성 관리, 빌드, 테스트, 실행 | 필수 |
| `rustc` | Rust 컴파일러 | 필수 |
| `rustfmt` | 코드 포맷 검사 | 필수 |
| `clippy` | Rust 정적 분석 | 필수 |
| `rust-analyzer` | IDE 타입 검사, 탐색, 자동 완성 | 필수 |
| Git | 소스 버전 관리 | 필수 |

`rustup`은 Rust와 Cargo를 함께 설치한다. 프로젝트는 stable 툴체인을 사용한다.

```bash
rustup toolchain install stable
rustup default stable
rustup component add rustfmt clippy
```

루트의 `rust-toolchain.toml`은 개발 환경과 CI가 같은 툴체인을 선택하게 한다.

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

이 프로젝트는 실행 파일을 만드는 애플리케이션이다. `Cargo.lock`을 Git에 커밋한다.

## 운영체제 빌드 도구

Rust 툴체인 외에 운영체제별 네이티브 도구가 필요하다.

| 운영체제 | 필요한 도구 |
|---|---|
| Windows | Visual Studio Build Tools의 MSVC 컴파일러와 Windows SDK |
| macOS | Xcode Command Line Tools |
| Linux | GCC 또는 Clang, `pkg-config`, X11 또는 Wayland 개발 패키지 |

Linux 패키지 이름은 배포판과 `eframe` 렌더링 기능에 따라 다르다. 지원 배포판을 정한 후 설치 목록을 별도 문서에 고정한다.

## 패키지 관리

Cargo가 프로젝트의 패키지 관리자다. 별도 패키지 관리자를 사용하지 않는다.

주요 명령은 다음과 같다.

```bash
cargo add tokio
cargo check
cargo build
cargo test
cargo run
cargo update
```

`cargo add`는 호환되는 크레이트 버전을 `Cargo.toml`에 추가한다. `Cargo.lock`은 실제 빌드에 사용한 버전을 고정한다.

`cargo-binstall`은 필수가 아니다. 개발용 Cargo 확장 도구의 사전 빌드 바이너리가 필요할 때만 사용한다.

## 코드 포맷과 정적 분석

CI는 다음 검사를 실행한다.

```bash
cargo fmt --all --check

cargo clippy \
	--workspace \
	--all-targets \
	--all-features \
	-- \
	-D warnings

cargo test --workspace
cargo doc --workspace --no-deps
```

`cargo fmt --check`는 포맷이 다른 코드를 거부한다. `cargo clippy`는 모든 경고를 오류로 처리한다.

핵심 크레이트는 안전하지 않은 Rust 코드를 금지한다.

```rust
#![forbid(unsafe_code)]
```

운영체제 API 연동에 `unsafe`가 필요하면 플랫폼 전용 모듈이나 크레이트에 격리한다. 네트워크 처리, 데이터 모델, 저장소, UI에는 `unsafe`를 허용하지 않는다.

## 의존성 검사

`cargo-deny`는 의존성의 라이선스, 금지 목록, 출처, 보안 권고를 검사한다.

```bash
cargo install --locked cargo-deny
cargo deny init
cargo deny check
```

초기 CI에는 `cargo-deny`만 사용한다. `cargo-audit`은 보안 권고 검사 역할이 겹치므로 별도로 추가하지 않는다.

## 테스트 실행기

초기에는 `cargo test`를 사용한다. 테스트 수가 늘어나면 `cargo-nextest`를 추가한다.

```bash
cargo nextest run
cargo test --doc
```

`cargo-nextest`는 doctest를 실행하지 않는다. CI는 `cargo test --doc`을 별도로 실행한다.

## MVP 필수 크레이트

### 비동기 런타임

```toml
tokio = { version = "1", features = [
	"rt-multi-thread",
	"macros",
	"net",
	"io-util",
	"sync",
	"time",
	"signal",
] }
```

Tokio는 다음 작업을 담당한다.

- TCP 리스너와 연결을 처리한다.
- 비동기 읽기와 쓰기를 처리한다.
- 제한된 채널로 기록 이벤트를 전달한다.
- 작업 취소, 종료 신호, 타임아웃을 처리한다.

프로젝트는 비동기 런타임을 Tokio 하나로 통일한다.

### HTTP 프록시

```toml
hyper = { version = "1", features = ["client", "server", "http1"] }
hyper-util = { version = "0.1", features = ["tokio"] }
http = "1"
http-body-util = "0.1"
bytes = "1"
```

각 크레이트의 역할은 다음과 같다.

| 크레이트 | 역할 |
|---|---|
| `hyper` | HTTP 클라이언트와 서버 연결 |
| `hyper-util` | Hyper 스트림과 Tokio 연결 |
| `http` | 요청, 응답, URI, 헤더 타입 |
| `http-body-util` | 스트리밍 본문 처리 |
| `bytes` | 네트워크 바이트 버퍼 |

첫 구현은 HTTP/1.1만 활성화한다. HTTP/2 구현을 시작할 때 `http2` 기능을 추가한다.

본문은 스트리밍으로 전달한다. 프록시는 크기를 제한하지 않은 본문을 메모리에 전부 모으지 않는다.

### Rust 네이티브 데스크톱 UI

```toml
eframe = "0.37"
```

`eframe`은 다음 요소를 통합한다.

- `egui` UI
- 네이티브 창과 입력 처리
- 렌더링 백엔드
- 클립보드와 접근성 연동

프로젝트는 `eframe::run_native`로 데스크톱 앱을 실행한다. `egui`, `winit`, `wgpu`를 각각 직접 추가하지 않는다. 커스텀 렌더링 요구가 생길 때만 하위 크레이트를 직접 사용한다.

### SQLite 저장소

```toml
rusqlite = { version = "0.43", features = ["bundled"] }
```

`bundled` 기능은 SQLite를 애플리케이션과 함께 빌드한다. 사용자 컴퓨터의 SQLite 설치 여부와 버전에 의존하지 않는다.

`rusqlite`는 동기 API를 제공한다. 저장소 전용 스레드 하나가 데이터베이스 쓰기를 소유한다. Tokio 작업은 제한된 채널로 저장 명령을 보낸다.

짧은 일회성 쿼리에 `tokio::task::spawn_blocking`을 사용할 수 있다. 같은 SQLite 연결을 여러 비동기 작업이 함께 소유하게 만들지 않는다.

### 직렬화

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Serde는 다음 데이터에 사용한다.

- 앱 설정
- HAR 내보내기
- 테스트 fixture
- 세션 메타데이터

검색과 정렬이 필요한 SQLite 필드는 정규 컬럼으로 저장한다. 데이터베이스 행 전체를 JSON으로 저장하지 않는다.

### 오류 타입

```toml
thiserror = "2"
```

각 모듈은 `thiserror`로 오류 타입을 정의한다.

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
	#[error("failed to bind proxy listener: {0}")]
	Bind(std::io::Error),

	#[error("upstream connection failed: {0}")]
	Upstream(std::io::Error),
}
```

핵심 모듈에는 `anyhow`를 사용하지 않는다. 호출자가 실패 종류를 구분할 수 있는 오류 타입을 반환한다.

### 로그와 진단

```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
```

네트워크 요청 로그는 다음 식별자를 span에 포함한다.

- 세션 ID
- 요청 ID
- 연결 ID
- 대상 호스트
- 처리 단계

로그에는 `Authorization`, Cookie, 요청 본문, 응답 본문을 기록하지 않는다.

### 식별자와 데이터 경로

```toml
uuid = { version = "1", features = ["v4", "serde"] }
directories = "6"
url = "2"
```

각 크레이트의 역할은 다음과 같다.

| 크레이트 | 역할 |
|---|---|
| `uuid` | 세션과 요청 식별자 생성 |
| `directories` | 운영체제별 앱 데이터 디렉터리 탐색 |
| `url` | URL 파싱과 검증 |

프록시는 URL을 문자열 연산으로 파싱하지 않는다.

## HTTPS 단계의 크레이트

HTTPS 복호화 구현은 다음 크레이트를 사용한다.

```toml
rustls = "0.23"
tokio-rustls = "0.26"
rustls-native-certs = "0.8"
rcgen = "0.14"
fs2 = "0.4"
sha2 = "0.10"
time = "0.3"
```

각 크레이트의 역할은 다음과 같다.

| 크레이트 | 역할 |
|---|---|
| `rustls` | TLS 구현과 인증서 검증 |
| `tokio-rustls` | Tokio 스트림의 비동기 TLS 연결 |
| `rustls-native-certs` | 운영체제의 신뢰된 루트 인증서 로드 |
| `rcgen` | 로컬 CA와 도메인별 인증서 생성 |
| `fs2` | 여러 프로세스의 CA 초기화 잠금 |
| `sha2` | UI에 표시할 CA 지문 계산 |
| `time` | 루트와 leaf 인증서 유효기간 계산 |

TLS 구현은 rustls로 통일한다. `openssl`과 `native-tls`를 함께 추가하지 않는다.

OS 신뢰 저장소에 CA를 설치하는 코드는 운영체제별 모듈에 둔다. 현재 앱은 인증서 경로와 지문만 표시한다. 설치와 제거 자동화는 [HTTPS 후속 작업](https-future-work.md)에 정의한다.

## MVP에서 제외하는 크레이트

다음 크레이트와 도구는 현재 범위에 필요하지 않다.

| 항목 | 제외 이유 |
|---|---|
| `reqwest` | 프록시 엔진이 Hyper를 직접 사용하므로 역할이 겹친다. |
| `axum` | 이 프로젝트는 HTTP API 서버가 아니다. |
| `tower` | 공통 미들웨어 요구가 아직 없다. |
| `sqlx` | 단일 SQLite writer에는 `rusqlite`가 더 작은 구성이다. |
| `openssl`, `native-tls` | rustls와 역할이 겹친다. |
| `wgpu`, `winit` | `eframe`이 통합한다. |
| `rayon` | CPU 병렬 작업 요구가 확인되지 않았다. |
| `parking_lot` | 표준 동기화 도구와 Tokio 채널로 시작한다. |
| HTTP/3과 QUIC 크레이트 | HTTP/1.1과 HTTPS를 완성한 후 검토한다. |
| 범용 플러그인 시스템 | 두 번째 캡처 소스가 생긴 후 공통 경계를 정한다. |

## 초기 `Cargo.toml`

다음 구성은 HTTP/1.1 MVP를 대상으로 한다.

```toml
[package]
name = "http-proxy"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
bytes = "1"
directories = "6"
eframe = "0.37"
http = "1"
http-body-util = "0.1"
hyper = { version = "1", features = ["client", "server", "http1"] }
hyper-util = { version = "0.1", features = ["tokio"] }
rusqlite = { version = "0.43", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = [
	"rt-multi-thread",
	"macros",
	"net",
	"io-util",
	"sync",
	"time",
	"signal",
] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
url = "2"
uuid = { version = "1", features = ["v4", "serde"] }

[dev-dependencies]
tempfile = "3"
```

HTTPS 복호화 단계에서 다음 항목을 추가한다.

```toml
rcgen = "0.14"
rustls = "0.23"
rustls-native-certs = "0.8"
tokio-rustls = "0.26"
```

## 초기 코드 구조

프로젝트는 하나의 Cargo 패키지와 내부 모듈로 시작한다.

```text
src/
├── main.rs
├── app.rs
├── domain/
│   ├── mod.rs
│   ├── exchange.rs
│   └── body.rs
├── proxy/
│   ├── mod.rs
│   ├── server.rs
│   ├── connect.rs
│   └── forwarding.rs
├── storage/
│   ├── mod.rs
│   ├── schema.rs
│   └── body_store.rs
├── ui/
│   ├── mod.rs
│   ├── request_table.rs
│   └── request_detail.rs
└── telemetry.rs
```

HTTPS 구현을 시작할 때 `tls` 모듈을 추가한다.

```text
src/tls/
├── mod.rs
├── authority.rs
├── certificate_cache.rs
└── upstream.rs
```

시스템 프록시와 CA 설치 기능은 `platform` 모듈에 둔다.

```text
src/platform/
├── mod.rs
├── windows.rs
├── macos.rs
└── linux.rs
```

각 운영체제 구현은 `cfg` 속성으로 분리한다. 공통 모듈이 운영체제 API 타입에 의존하지 않게 한다.

## CI 최소 검사

CI는 지원 운영체제마다 다음 명령을 실행한다. 이벤트별로 생략하는 러너와 단계는 [개발 문서의 CI](development.md#ci)에 있다.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo deny check
cargo build --release
```

Windows, macOS, Linux를 지원하면 각 운영체제에서 빌드한다. 인증서 저장소와 시스템 프록시 기능은 운영체제별 통합 테스트가 필요하다.

## 최소 구성 요약

첫 HTTP 프록시 구현에 필요한 구성은 다음과 같다.

```text
rustup, cargo
rustfmt, clippy, rust-analyzer
tokio, hyper, hyper-util, http-body-util
eframe
rusqlite
serde
thiserror
tracing
```

HTTPS 복호화를 시작할 때 다음 크레이트를 추가한다.

```text
rustls, tokio-rustls, rustls-native-certs, rcgen
fs2, sha2, time
```

## 참고 자료

- [Cargo 설치](https://doc.rust-lang.org/cargo/getting-started/installation.html)
- [rustfmt Cargo 명령](https://doc.rust-lang.org/nightly/cargo/commands/cargo-fmt.html)
- [Clippy 설치](https://doc.rust-lang.org/clippy/installation.html)
- [Hyper 스트리밍 본문](https://docs.rs/hyper/latest/hyper/body/)
- [eframe 네이티브 앱](https://docs.rs/eframe/latest/eframe/)
- [rusqlite와 bundled SQLite](https://docs.rs/crate/rusqlite/latest)
- [tokio-rustls](https://docs.rs/crate/tokio-rustls/latest)
- [cargo-deny](https://embarkstudios.github.io/cargo-deny/)
- [cargo-nextest](https://www.nexte.st/)
