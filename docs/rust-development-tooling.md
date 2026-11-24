# Rust 개발 도구 및 의존성 기준

개발 환경 설치와 로컬 검사 명령은 [개발 문서](development.md)에 있다.

## 설정 파일

| 파일 | 관리하는 항목 |
| --- | --- |
| [`rust-toolchain.toml`](../rust-toolchain.toml) | Rust 1.100.0, `rustfmt`, Clippy |
| [`Cargo.toml`](../Cargo.toml) | 워크스페이스 멤버, Rust 2024 에디션, 공유 의존성, lint, Release 프로필 |
| [`Cargo.lock`](../Cargo.lock) | 앱과 워크스페이스 전체가 사용하는 의존성 버전 |
| [`clippy.toml`](../clippy.toml) | 함수 길이와 복잡도 기준, 금지한 메서드와 타입 |
| [`deny.toml`](../deny.toml) | 보안 권고, 라이선스, 중복 버전과 의존성 출처 검사 |
| [`ci.yml`](../.github/workflows/ci.yml) | 플랫폼별 검증과 배포 |

## 구성 요소와 의존성

| 구성 요소 | 크레이트와 역할 |
| --- | --- |
| 앱 화면 | `eframe`과 `egui_extras`로 데스크톱 UI를 그린다. eframe은 Linux에서 X11을 사용하며 OpenGL 렌더링과 AccessKit을 활성화한다. |
| 프록시 | `tokio`로 연결을 처리하고 `hyper`와 `hyper-util`로 HTTP/1.1 요청·응답을 스트리밍한다. |
| HTTPS | `rustls`, `tokio-rustls`로 양쪽 TLS 연결을 처리한다. `rcgen`으로 로컬 CA와 서버 이름별 인증서를 발급하고 `rustls-native-certs`로 업스트림 검증에 쓸 시스템 루트를 읽는다. |
| 캡처와 저장 | `http` 타입을 바탕으로 캡처 레코드를 구성한다. 세션 전체 로그와 본문은 임시 파일에 기록하고 화면 목록은 메모리에 유지한다. |
| 본문 표시와 검색 | `flate2`, `brotli-decompressor`, `zstd`로 압축을 푼다. `regex-automata`로 정규식 검색을 하고 `resvg`, `png`, `jpeg-decoder` 등으로 이미지 미리보기를 만든다. |
| 내보내기 | `serde_json`으로 HAR, URL 맵과 일괄 본문 저장의 JSON 출력을 작성한다. |
| 진단과 성능 | `tracing`과 `tracing-subscriber`로 진단 로그를 남긴다. `criterion`으로 검색과 URL 맵을 측정한다. |

각 의존성이 속하는 크레이트와 허용된 참조 방향은 [워크스페이스 구조와 의존 규칙](development.md#워크스페이스-구조와-의존-규칙)을 참고한다.
