# 캡처와 저장

[개발자 가이드](README.md)

응답 헤더를 받은 시점, 본문 수신이 끝난 시점, 모든 바이트를 보관했는지는 각각 다른 정보다. 예를 들어 200 응답의 본문이 완전히 전달되어도 미리보기는 앞부분만 보관할 수 있다.

## 레코드의 상태

| 상태 | 답하는 질문 | 코드 |
| --- | --- | --- |
| `Outcome` | 요청이 대기 중인가, 응답 헤더가 왔는가, 터널이 열렸는가, 또는 실패했는가? | [`Exchange`와 `Outcome`](../../crates/http-inspector-exchange/src/exchange/record.rs) |
| `BodyLifecycle` | 이 방향의 본문이 수신 중인가, 완료됐는가, 도중에 끊겼거나 취소됐는가? | [본문 수명과 미리보기](../../crates/http-inspector-exchange/src/exchange.rs) |
| `BodyRetention` | 미리보기에서 바이트를 얼마나 보관했는가? | [미리보기 보존 상태](../../crates/http-inspector-exchange/src/exchange.rs) |

프록시의 `CaptureContext`가 공유 레코드의 쓰기 핸들을 소유한다. UI와 분석 코드는 `Exchange`와 본문 스냅샷을 읽는다. 수신을 마친 본문과 미리보기에서 전체를 읽을 수 있는 본문을 구분하려면 수명과 보존 상태를 함께 확인한다. [쓰기 핸들 소유권](../development.md#경계-검사)

## 바이트가 저장되는 세 곳

`ObservedBody`는 프레임을 관찰할 때 미리보기를 갱신하고 로그와 본문 저장소에도 데이터를 넘긴다. 활성화된 각 저장 경로는 자체 한도와 상태를 관리한다. [본문 관찰 코드](../../crates/http-inspector-core/src/interception/capture.rs)

| 저장 경로 | 구현과 주요 소비자 |
| --- | --- |
| 메모리 미리보기 | `BodyPreview`가 제한된 바이트와 진행 상태를 갖는다. 상세 화면, 검색, HAR이 캡처된 미리보기를 사용한다. [캡처 한도](../../crates/http-inspector-exchange/src/capture.rs) |
| 전체 로그 | `FullLog`가 요청·응답 헤더, 본문, 결과를 세션 임시 파일에 기록한다. 큐가 차면 자리가 날 때까지 해당 기록 쓰기가 대기한다. **Save all logs**는 그 시점의 저널을 내보낸다. [저널](../../crates/http-inspector-core/src/recording/journal.rs) |
| 파일 저장용 본문 | `BodyStore`가 레코드 ID와 방향별로 바이트를 보관한다. 저장 경로가 한도를 만나면 해당 본문을 불완전으로 표시하고 프록시는 전달을 계속한다. 개별·일괄 본문 저장과 범위 합치기가 이 저장소를 읽는다. [본문 저장소](../../crates/http-inspector-core/src/recording/body_store.rs) |

**Clear**는 요청 목록과 URL 맵의 상태를 비운다. 전체 로그와 본문 저장소는 앱 세션의 상태로 유지한다. 각 한도와 저장 절차는 [창, 로그와 본문 저장](../guide/logs.md)에 있다.

## 기능이 읽는 데이터

- **HAR와 cURL 복사.** 화면에서 선택한 캡처 레코드를 `http-inspector-interop`에 넘긴다. HAR은 설정에 따라 본문 미리보기를 포함한다. cURL 복사는 요청 본문을 온전히 재현할 수 있는지 검사한다. [HAR](../../crates/http-inspector-interop/src/har/mod.rs) · [cURL](../../crates/http-inspector-interop/src/curl.rs)
- **엔드포인트의 본문 구조 분석.** 분석 작업은 `BodySource`를 통해 크기 제한 안에서 완전한 본문을 압축 해제해 읽는다. `BodyStore` 구현은 미리보기의 바이트가 충분하면 메모리에서, 부족하면 저장 파일에서 읽는다. 저장소 없이 시작한 세션은 미리보기만 사용하는 `PreviewBodies`를 제공한다. 파일 읽기나 쓰기 대기가 있을 수 있으므로 분석 작업은 백그라운드에서 읽는다. [`BodySource`](../../crates/http-inspector-exchange/src/exchange/body_source.rs) · [저장소 구현](../../crates/http-inspector-core/src/recording/body_store.rs)

민감 헤더의 값을 가리는 규칙은 공유 [HTTP 헤더 처리 코드](../../crates/http-inspector-exchange/src/http_semantics/sensitive.rs)에 있다. 값 가림은 URL이나 본문에 적용되지 않는다. 헤더 캡처를 바꿀 때는 [전체 로그 테스트](../../tests/full_log.rs)와 [HAR 테스트](../../tests/har_export.rs)도 확인한다.
