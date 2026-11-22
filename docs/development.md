# 개발 문서

[README](../README.md)로 돌아가기

이 문서는 HTTP Inspector를 개발하고 검증할 때 필요한 환경, 명령과 설계 문서를 모은다. 작업 흐름, 커밋과 PR 규칙은 [AGENTS.md](../AGENTS.md)를 따른다.

## 개발 환경

프로젝트는 Rust 1.100.0을 사용한다. `rustup`은 `rust-toolchain.toml`에 지정된 툴체인을 설치한다. Linux에서는 X11을 사용하며, macOS에서는 X11이 필요하지 않다.

Linux 시스템 패키지와 macOS의 선택 도구(Homebrew NSS)는 [README의 설치](../README.md#설치)를 따른다. CI도 같은 패키지를 설치한다.

글꼴 등록을 확인하려면 다음처럼 실행한다. 어떤 폰트를 등록했는지 로그로 확인할 수 있다.

```bash
RUST_LOG=http_proxy::ui::fonts=debug cargo run
```

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

macOS 실환경에서 키체인 등록·상태 확인·제거와 Chrome 검사 창 실행을 확인하려면 다음 검사를 실행한다. 첫 검사는 임시 CA를 로그인 키체인에 등록한 뒤 제거하며, macOS 인증 창이 나타날 수 있다.

```bash
cargo test --test macos_integration -- --ignored
```

## 설계 문서

- [네트워크 인스펙터 설계](http-network-inspector-design.md)
- [Rust 개발 도구 및 의존성](rust-development-tooling.md)
- [코드 품질 기준](code-quality.md)
- [Network 상세 보기 설계](f12-network-increment-design.md)
- [HTTPS 복호화 구현 설계](https-inspection-implementation.md)
- [HTTPS 후속 작업](https-future-work.md)

Git 작업 흐름은 별도 문서 없이 [AGENTS.md](../AGENTS.md)의 Git·커밋·PR 지침에 정리되어 있다.
