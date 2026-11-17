# HTTP Inspector

HTTP Inspector는 로컬 HTTP 프록시를 통해 요청 기록을 보여주는 Linux 데스크톱 MVP다. UI와 프록시 엔진은 모두 Rust로 작성했다.

## MVP 기능

현재 버전은 다음 기능을 제공한다.

- `127.0.0.1:8080`의 HTTP 프록시
- HTTP/1.1 요청과 응답 스트리밍
- HTTPS `CONNECT` TCP 터널
- 메서드, 대상, 결과, 응답 헤더 도착 시간 표시
- 프록시 시작, 중지, 기록 삭제
- 최대 2,000개의 메모리 기록
- 기록 큐 초과 건수 표시

## 제외 범위

현재 버전은 다음 기능을 제공하지 않는다.

- HTTPS 복호화와 로컬 CA 설치
- 요청과 응답 본문 저장
- 영구 기록과 SQLite 저장소
- 시스템 프록시 자동 변경
- HTTP/2와 HTTP/3
- WebSocket 프레임 분석
- Wayland와 화면 읽기 프로그램 지원

HTTPS 요청은 `CONNECT` 터널로 전달한다. 앱은 터널 안의 URL, 헤더, 본문을 읽지 않는다.

## Linux 요구사항

프로젝트는 Rust 1.100.0과 X11을 사용한다. `rustup`은 `rust-toolchain.toml`에 지정된 툴체인을 설치한다.

Ubuntu 계열 배포판에서 CI와 같은 개발 패키지를 설치하려면 다음 명령을 사용한다.

```bash
sudo apt-get update
sudo apt-get install --yes \
	libgl1-mesa-dev \
	libx11-dev \
	libxkbcommon-dev \
	libxkbcommon-x11-dev \
	libxcursor-dev \
	libxi-dev \
	libxrandr-dev \
	pkg-config
```

## 실행

저장소 루트에서 앱을 실행한다.

```bash
cargo run --release
```

UI에 `Running`과 `127.0.0.1:8080`이 표시되면 프록시가 요청을 받을 수 있다.

HTTP 요청을 확인하려면 `curl`에 프록시 주소를 지정한다.

```bash
curl --proxy http://127.0.0.1:8080 http://example.com/
```

환경 변수로 명령줄 프로그램에 프록시를 지정할 수도 있다.

```bash
HTTP_PROXY=http://127.0.0.1:8080 your-command
HTTPS_PROXY=http://127.0.0.1:8080 your-command
```

프로그램이 `NO_PROXY`를 설정하면 로컬 주소나 지정된 호스트가 프록시를 우회할 수 있다.

## 검증

전체 로컬 검사를 실행한다.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo deny check
cargo build --release
```

통합 테스트는 실제 TCP 연결을 사용한다. 테스트는 HTTP 전달과 `CONNECT` 터널의 양방향 전송을 검증한다.

## 문서

- [네트워크 인스펙터 설계](docs/http-network-inspector-design.md)
- [Rust 개발 도구 및 의존성](docs/rust-development-tooling.md)
- [코드 품질 기준](docs/code-quality.md)
- [Git 작업 흐름](docs/git-workflow.md)

## 라이선스

이 프로젝트는 [MIT License](LICENSE)를 따른다.
