# HTTP Inspector

HTTP Inspector는 로컬 HTTP 프록시를 통해 요청 기록을 보여주는 Linux 데스크톱 MVP다. UI와 프록시 엔진은 모두 Rust로 작성했다.

## MVP 기능

현재 버전은 다음 기능을 제공한다.

- `127.0.0.1:8080`의 HTTP 프록시
- HTTP/1.1 요청과 응답 스트리밍
- HTTPS `CONNECT` TCP 터널
- 사용자가 켤 수 있는 HTTPS 복호화
- 설치별 로컬 CA 생성과 제한된 개인키 저장
- 실제 HTTPS 서버 인증서와 호스트 이름 검증
- 메서드, 대상, 결과, 응답 헤더 도착 시간 표시
- 요청과 응답 헤더 상세 보기
- 방향별 최대 8KiB의 실시간 본문 미리보기
- 문자열 검색과 응답, 실패, 터널 결과 필터
- 선택한 요청의 Headers와 Body 상세 패널
- 프록시 시작, 중지, 기록 삭제
- 최대 2,000개의 메모리 기록
- 기록 큐 초과 건수 표시

## 제외 범위

현재 버전은 다음 기능을 제공하지 않는다.

- 브라우저 신뢰 저장소 자동 변경
- 전체 요청과 응답 본문 저장
- 영구 기록과 SQLite 저장소
- 시스템 프록시 자동 변경
- HTTP/2와 HTTP/3
- WebSocket 프레임 분석
- Wayland와 화면 읽기 프로그램 지원

HTTPS 복호화는 기본으로 꺼져 있다. 이 상태에서 앱은 `CONNECT` 터널 안의 URL, 헤더와 본문을 읽지 않는다. 사용자가 복호화를 켜면 앱은 HTTP/1.1 HTTPS 요청을 기존 상세 패널에 표시한다.

본문 미리보기는 메모리에만 유지하며 기록을 삭제하거나 앱을 종료하면 사라진다. `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie` 헤더 값은 캡처 전에 가린다. 큰 본문은 전체를 계속 전달하지만 미리보기만 8KiB에서 자른다.

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

## HTTPS 복호화

HTTPS 복호화는 사용 권한이 있는 트래픽에만 사용한다.

1. 실행 중인 프록시에서 **Stop**을 누른다.
2. **Decrypt HTTPS**를 선택한다.
3. **Start**를 누른다.
4. UI에 표시된 `root-ca.pem` 경로와 SHA-256 지문을 확인한다.
5. Chrome의 `chrome://settings/certificates`에서 공개 CA 인증서를 신뢰 기관으로 가져온다.
6. Chrome을 완전히 종료한 후 프록시 설정과 함께 다시 실행한다.

기본 CA 경로는 다음과 같다.

```text
$XDG_DATA_HOME/http-inspector/ca/root-ca.pem
```

`XDG_DATA_HOME`이 없으면 다음 경로를 사용한다.

```text
$HOME/.local/share/http-inspector/ca/root-ca.pem
```

Chrome의 HTTP/3 우회를 막으려면 테스트 프로필을 다음과 같이 실행한다.

```bash
google-chrome \
	--user-data-dir=/tmp/http-inspector-chrome \
	--proxy-server=http://127.0.0.1:8080 \
	--disable-quic
```

Chrome에는 `root-ca.pem`만 등록한다. `ca-identity.pem`에는 개인키가 있으므로 열거나 복사하거나 신뢰 저장소에 등록하지 않는다. 앱을 삭제해도 Chrome에 등록한 CA는 남는다. 현재 버전에서는 Chrome 설정에서 CA를 직접 제거해야 한다.

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
- [Network 상세 보기 설계](docs/f12-network-increment-design.md)
- [HTTPS 복호화 구현 설계](docs/https-inspection-implementation.md)
- [HTTPS 후속 작업](docs/https-future-work.md)

## 라이선스

이 프로젝트는 [MIT License](LICENSE)를 따른다.
