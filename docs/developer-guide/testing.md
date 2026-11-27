# 테스트 선택

[개발자 가이드](README.md)

프록시 시작, 소켓 전달 또는 종료가 바뀌면 통합 테스트에서 시작한다. 포맷, 검색 규칙, 내보내기 형식처럼 특정 입력에 대한 출력이 바뀌면 해당 크레이트 테스트부터 확인한다. 명령은 저장소나 작업 워크트리 루트에서 실행한다.

## 첫 요청의 상태 전이 확인

[요청의 수명](request-lifecycle.md)에 나오는 대기 중 요청을 테스트에서 볼 수 있다.

```bash
cargo test --test exchange_lifecycle publishes_an_exchange_before_its_response_arrives
```

테스트는 루프백 서버를 띄우고 요청을 받되 응답은 잠시 보류한다. 프록시에서 `InFlight` 레코드를 받은 뒤에야 서버의 응답을 허용한다. 클라이언트에 200 응답이 오고 이미 받은 레코드가 `Response`로 바뀌는지 확인한다. [테스트 코드](../../tests/exchange_lifecycle.rs)

## 변경한 동작에 맞는 테스트

| 변경 범위 | 테스트 | 확인하는 동작 |
| --- | --- | --- |
| HTTP 전달, CONNECT, TLS 검증, 재전송 | [`proxy_behavior`](../../tests/proxy_behavior.rs) · [`proxy_edge_cases`](../../tests/proxy_edge_cases.rs) | 업스트림 요청과 클라이언트 응답, 터널 연결, 인증서 검증, 설정된 타임아웃과 종료 |
| 레코드 게시, 타이밍, 본문 스트리밍 | [`exchange_lifecycle`](../../tests/exchange_lifecycle.rs) · [`body_completion`](../../tests/body_completion.rs) | 응답 전 레코드 게시, 취소·끊김 구분, 본문 종료 전 청크 전달 |
| 전체 로그와 파일 저장 | [`full_log`](../../tests/full_log.rs) · [`body_store`](../../tests/body_store.rs) | 화면 보관과 독립적인 로그, 저장한 바이트, 불완전 파일, 범위 응답 합치기 |
| 목록 검색과 분석 | [`search_parity`](../../tests/search_parity.rs) · [`url_map_integration`](../../tests/url_map_integration.rs) · [`endpoint_schema_integration`](../../tests/endpoint_schema_integration.rs) | 테스트용 레코드와 실제 캡처의 검색 일치, URL 맵과 엔드포인트 집계 |
| HAR, cURL | [`har_export`](../../tests/har_export.rs) · [`copy_as_curl`](../../tests/copy_as_curl.rs) | 캡처를 소비하는 내보내기의 본문, 헤더, 형식 처리 |
| UI 번역 카탈로그와 툴팁 정책 | [`ui::i18n::tests`](../../src/ui/i18n/tests.rs) · [`check-boundaries.py`](../../scripts/check-boundaries.py) | 정적 카탈로그 비어있지 않음, 툴팁 `S1` 한 문장 규칙, `src/ui/i18n/` 바깥 인라인 번역 차단 |

표의 통합 테스트는 `cargo test --test <이름>`으로 실행하고, 크레이트 단위 테스트는 `cargo test -p http-inspector-analysis`처럼 패키지 이름을 지정한다. UI 번역 정책 단위 테스트와 경계 검사는 `cargo test ui::i18n::tests`와 `python3 scripts/check-boundaries.py`로 실행한다.

## 테스트에 데이터 제공

- 정책이나 변환을 확인하는 데 캡처 데이터만 필요하면 `test-support`의 `ExchangeBuilder`로 레코드를 만든다. 엔드포인트 본문 해석을 테스트할 때는 `MemoryBodySource`에 디코딩된 바이트를 제공한다. [레코드 빌더](../../crates/http-inspector-exchange/src/exchange_builder.rs) · [메모리 본문 소스](../../crates/http-inspector-exchange/src/exchange/body_source.rs)
- 프록시가 실제로 어떤 바이트를 보내고 상태를 캡처하는지 확인할 때는 임의의 루프백 포트와 테스트 업스트림을 사용한다. [통합 테스트 공통 코드](../../tests/common/mod.rs)
- macOS 키체인과 설치된 Chrome 검사 창은 macOS 실환경 테스트에서 확인한다. 실행 조건과 전체 로컬 검사 명령은 [개발 문서](../development.md#검증)에 있다.
