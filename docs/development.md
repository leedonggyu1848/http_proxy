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

## 배포 검증

CI는 Ubuntu 24.04 x86_64, macOS 15 arm64, macOS 15 Intel에서 각각 네이티브 빌드한다. `scripts/package.py`는 실행 파일과 공개 문서·라이선스만 복사해 tar.gz와 SHA-256 파일을 만든다. `scripts/smoke-package.py`는 Actions에서 다시 다운로드한 압축의 체크섬과 실행 권한을 확인하고, 임시 데이터 디렉터리에서 GUI를 실행해 로컬 HTTP 요청을 전달한다. Linux는 Xvfb를 사용한다. 성공 로그에 실제 OS와 CPU 아키텍처가 남는다. 서명·공증, Gatekeeper 승인과 사용자 화면 조작은 검사하지 않는다.

릴리스를 만들 때는 `Cargo.toml`의 버전과 정확히 일치하는 `v<버전>` 태그를 push한다. 태그 검증, 세 대상의 품질 검사·빌드·실행 검사와 의존성 검사가 모두 성공해야 릴리스 job이 실행된다. 이 job은 세 압축과 체크섬을 확인하고 draft에 모두 첨부한 다음 공개한다. 실패한 draft는 정상 릴리스로 공개하지 않는다. 같은 태그의 릴리스가 이미 있으면 자동으로 덮어쓰지 않는다. 재실행 전에 해당 draft의 첨부 상태를 확인하고 필요하면 draft를 삭제한다. 이미 공개된 릴리스는 새 버전으로 수정한다.

Actions의 [파일 권한 제한](https://github.com/actions/upload-artifact/tree/v4#permission-loss) 때문에 실행 파일을 tar.gz로 감싼다. 대상은 GitHub의 [호스팅 runner 목록](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)에 따라 고정하고, 공개에는 GitHub CLI의 [draft와 verify-tag 옵션](https://cli.github.com/manual/gh_release_create)을 사용한다.

## 설계 문서

- [네트워크 인스펙터 설계](http-network-inspector-design.md)
- [Rust 개발 도구 및 의존성](rust-development-tooling.md)
- [코드 품질 기준](code-quality.md)
- [Network 상세 보기 설계](f12-network-increment-design.md)
- [HTTPS 복호화 구현 설계](https-inspection-implementation.md)
- [HTTPS 후속 작업](https-future-work.md)

Git 작업 흐름은 별도 문서 없이 [AGENTS.md](../AGENTS.md)의 Git·커밋·PR 지침에 정리되어 있다.
