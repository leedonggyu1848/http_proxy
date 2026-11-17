# Git 작업 흐름

이 문서는 저장소의 브랜치, 커밋, 검증, PR 규칙을 정의한다. 모든 변경은 `origin`의 PR을 통해 `main`에 병합한다.

## 브랜치

`main`에 직접 커밋하지 않는다. 최신 `main`에서 작업 브랜치를 만든다.

브랜치 이름은 변경 목적을 나타낸다.

```text
feat/linux-http-inspector-mvp
fix/connect-half-close
docs/proxy-setup
chore/update-rust-toolchain
```

브랜치 접두사는 다음 기준을 따른다.

| 접두사 | 용도 |
|---|---|
| `feat/` | 사용자 기능 추가 |
| `fix/` | 동작 오류 수정 |
| `docs/` | 문서만 변경 |
| `refactor/` | 외부 동작을 바꾸지 않는 구조 변경 |
| `test/` | 테스트만 추가하거나 수정 |
| `chore/` | 툴체인, CI, 의존성, 저장소 관리 |

## 작업 단위

한 커밋은 한 가지 검토 가능한 변경을 담는다. 다음 작업을 한 커밋에 섞지 않는다.

- 저장소 설정 변경
- 도메인 모델 추가
- 프록시 전달 기능 추가
- UI 기능 추가
- 관련 없는 리팩터링

각 작업 단위는 다음 순서로 진행한다.

1. 변경 전 상태가 깨끗한지 확인한다.
2. 한 가지 목적의 파일만 수정한다.
3. 해당 단위에 필요한 검사를 실행한다.
4. diff에 비밀값과 임시 파일이 없는지 확인한다.
5. 검사가 통과하면 커밋한다.

다음 작업은 앞 작업의 검사가 통과한 후 시작한다.

## 커밋 메시지

커밋 제목은 다음 형식을 사용한다.

```text
<type>: <imperative summary>
```

허용하는 `type`은 `feat`, `fix`, `docs`, `refactor`, `test`, `chore`다.

제목은 변경 결과를 영어 명령형으로 적는다. 제목 끝에 마침표를 붙이지 않는다.

```text
docs: define code quality gates
feat: forward HTTP requests through the proxy
test: cover bidirectional CONNECT tunneling
```

커밋 본문은 결정 이유가 diff에 드러나지 않을 때만 작성한다. 파일 목록이나 테스트 출력 전체를 본문에 복사하지 않는다.

## 커밋 전 검사

문서만 수정한 커밋은 다음 검사를 실행한다.

```bash
git diff --check
```

Rust 코드를 수정한 커밋은 최소한 다음 검사를 실행한다.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

의존성을 바꾼 커밋은 `cargo deny check`도 실행한다. PR을 올리기 전에는 [코드 품질 기준](code-quality.md)의 전체 검사를 실행한다.

## PR

작업 브랜치를 `origin`에 push한 후 PR을 만든다.

```bash
git push -u origin <branch>

gh pr create \
	--base main \
	--head <branch> \
	--title "<title>" \
	--body-file <body-file>
```

PR 설명에는 다음 내용을 포함한다.

- 변경한 사용자 동작
- 의도적으로 제외한 범위
- 실행한 검증 명령
- 보안이나 호환성 주의사항

PR은 모든 필수 CI가 통과한 후 병합한다. 실패한 검사를 건너뛰거나 경고를 숨겨 병합하지 않는다.

## 리뷰 후 수정

리뷰 수정은 기존 커밋의 목적에 따라 처리한다.

- 아직 공유하지 않은 브랜치에서는 관련 커밋을 고칠 수 있다.
- 이미 리뷰 중인 브랜치에서는 검토하기 쉬운 후속 커밋을 추가한다.
- PR 기록을 숨기기 위한 강제 push는 하지 않는다.

병합 전략은 squash merge를 기본값으로 사용한다. PR 안의 작업 단위 커밋은 리뷰와 문제 추적을 위해 유지한다.

## 금지 사항

다음 파일과 값은 커밋하지 않는다.

- `target` 빌드 산출물
- IDE의 사용자 설정
- 로컬 로그와 프로파일 데이터
- 비밀번호, 토큰, 인증서 개인키
- 실제 세션의 HTTP 요청과 응답 본문
- 로컬 감사 로그

애플리케이션 프로젝트의 `Cargo.lock`은 커밋한다. 재현 가능한 빌드를 위해 `.gitignore`에 넣지 않는다.
