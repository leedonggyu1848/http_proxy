# 프로젝트 목표

HTTP Inspector는 로컬 프록시를 통해 HTTP 요청·응답과 선택적으로 복호화한 HTTPS 트래픽을 확인하는 Linux 데스크톱 앱이다. 사용자가 네트워크 동작을 정확히 파악할 수 있도록 전달의 정확성, 이해하기 쉬운 화면을 우선한다.

## 개발 환경

- Rust 1.100.0, edition 2024, Cargo를 사용한다. 툴체인은 `rust-toolchain.toml`을 따른다.
- UI는 eframe/egui와 X11, 프록시는 Tokio와 Hyper, TLS는 rustls를 사용한다.
- 명령은 저장소 루트 또는 작업 워크트리 루트에서 실행한다.
- 개발 환경, 검증 명령과 설계 문서 목록은 `docs/development.md`에 있다.
- Linux 시스템 패키지는 README의 설치 명령을 따른다. 기본 실행에는 별도 외부 서비스가 필요하지 않으며, UI 확인에는 X11 디스플레이가 필요하다.
- `XDG_DATA_HOME` 또는 `HOME`은 CA 저장 위치를 결정한다. CA 개인키 파일은 열어 출력하거나 공유하지 않는다.

개발 중에는 다음 명령으로 실행한다.

```bash
cargo run
```

릴리스 바이너리는 다음 명령으로 빌드한다.

```bash
cargo build --release
```

릴리스 모드로 확인하려면 다음 명령으로 실행한다.

```bash
cargo run --release
```

## 개발 테스트

- 문서만 변경하면 `git diff --check`를 실행한다.
- Rust 코드를 바꾸면 다음 검사를 실행한다.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

- 의존성을 바꾸면 `cargo deny check`도 실행한다. `cargo-deny`가 없다면 `cargo install --locked cargo-deny`로 설치한다.
- PR 전에는 CI와 별도로 로컬에서 아래 전체 검증을 실행한다. PR CI가 생략한 러너·단계(`macos-15-intel`, macOS Release 빌드·패키지 검사)는 `main` 푸시·일정 실행에서 검증한다. CI 실행 범위는 `docs/development.md`의 "CI"에 있다.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo deny check
cargo build --release
```

## 이슈 해결 지침

이슈 해결 요청을 받은 메인 agent는 직접 코드를 쓰지 않고 아래 순서로 subagent를 조율한다.

1. **작성**: 작성 agent 하나를 생성해 브랜치·워크트리 생성부터 구현, 검증, PR 생성까지 맡긴다.
2. **리뷰**: PR이 생성되면 리뷰 agent를 새로 생성해 PR을 리뷰하게 한다.
   - 문제가 없으면 리뷰 agent가 PR을 바로 머지한다.
   - 문제가 있으면 리뷰 agent가 PR에 코멘트를 남기고 머지하지 않는다.
3. **수정**: 코멘트가 남으면 새 agent를 만들지 않고 1단계의 같은 작성 agent에게 메시지를 보내, 코멘트를 읽고 해결해 같은 PR에 push하게 한다.
4. **반복**: PR이 머지될 때까지 2~3단계를 반복한다. 리뷰는 매번 새 리뷰 agent가 맡는다.

## 코드 작성 지침

- 작성된 코드를 탐색할 때에는 `.agents/skills/graphify` 스킬을 사용한다.

- 함수·타입·모듈은 명확한 책임 하나를 가진다.
- 외부 입력은 경계에서 검증하고 내부에는 도메인 타입을 전달한다.

## UI 디자인 지침

- `src/ui`의 화면, 위젯, 스타일, 화면 문구를 바꾸는 작성 agent(이슈 해결 지침 1·3단계)와 그 PR을 리뷰하는 리뷰 agent(2단계)는 작업 전에 다음 두 문서를 읽고 따른다.
  - 범용 슬롭 방지 규칙(`S` 번호)과 캡처 점검 절차: `.agents/skills/ui-unslop/SKILL.md`
  - 이 프로젝트의 디자인 시스템과 전용 규칙(`D` 번호): [docs/ui-design-system.md](docs/ui-design-system.md)
- 작성 agent는 PR 본문에 두 문서의 점검 결과와 캡처를 넣는다. 리뷰 agent는 규칙 위반을 규칙 번호로 인용해 코멘트하고 머지하지 않는다.

## Git과 커밋 지침.

- 깨끗한 `main`에서 `git pull --ff-only origin main`으로 갱신한다. 기존 작업 때문에 갱신할 수 없으면 강제 초기화나 임의 stash를 하지 말고 상태를 보고한다.
- 새로운 코드 작업은 최신 `main`에서 별도 브랜치와 워크트리를 만들어 진행한다. `main`에 직접 커밋하지 않는다.
- diff와 스테이징 내용을 확인하고 해당 작업의 변경만 커밋한다. 사용자의 기존 변경과 비밀정보를 포함하지 않는다.
- 커밋 제목은 기존 `<type>: <imperative summary>` 형식과 영어 명령형을 유지한다.
- 구현·설계·의존성 등에서 선택한 사항은 커밋 본문에 무엇을 선택했고 왜 선택했는지 커밋메시지에 짧게 기록한다. 대안을 검토했다면 선택을 가른 제약이나 장단점도 짧게 적는다.
- 선택이 없는 단순 수정에는 이유를 억지로 만들지 않는다.
- 작업이 끝나면 PR을 만든다.

## PR 지침

- 사용자가 체감하는 동작을 바꾸면 `CHANGELOG.md`의 `## [Unreleased]`에 항목을 추가한다. 작성 규칙과 릴리스 절차는 `docs/development.md`의 "릴리스와 버전"에 있다.
- PR 제목과 본문은 한국어로 쓴다. 변경한 사용자 동작, 변경 이유, 실행한 검증, 보안·호환성 영향을 설명한다.
- 리뷰어가 변경을 이해할 수 있도록 차트·도식 또는 실제 캡처를 본문에 함께 넣는다. UI 변경은 실제 화면 캡처를, 전달 흐름이나 상태 변경은 Mermaid 도식을 우선한다. 각 자료에는 무엇을 보여 주는지 짧게 설명한다.
- 성능 차트에는 실제 측정값과 조건을 쓴다. 확인하지 않은 화면이나 수치를 만들지 않는다. 캡처에는 테스트용 트래픽을 사용하고 민감정보가 보이지 않는지 확인한다.
- PR이 병합되면 해당 작업의 워크트리와 로컬·원격 브랜치를 정리한다. 미커밋 변경이나 다른 작업이 남았으면 삭제하지 않는다.