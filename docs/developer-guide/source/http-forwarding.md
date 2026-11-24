# `forward.rs`에서 HTTP 헤더와 본문 전달 따라가기

[개발자 가이드](../README.md) · [`forward.rs` 열기](../../../crates/http-inspector-core/src/interception/forward.rs)

이 파일에서 가장 중요한 시점은 업스트림이 응답 헤더를 보낸 순간이다. 프록시는 이때 상태 코드와 헤더를 캡처한 뒤, 본문이 계속 도착하는 동안 클라이언트로 전달한다. 평문 HTTP와 복호화된 HTTPS가 이 전달 경로를 공유한다.

## 1. `forward_http`가 평문 목적지를 준비한다

`upstream_target`이 요청 대상에서 호스트와 포트에 해당하는 `authority`와 서버에 보낼 `path`를 구한다. 파싱이나 검증에 실패하면 `FailureStage::Request`와 400을 반환한다. 성공하면 기본 포트 80으로 `connect_upstream`을 호출한다. DNS와 TCP 연결을 마친 경우에만 그 시간을 기록하고, 연결과 경로를 `forward_over_stream`에 넘긴다. [평문 HTTP 진입점](../../../crates/http-inspector-core/src/interception/forward.rs#L18) · [업스트림 연결](../../../crates/http-inspector-core/src/interception/upstream.rs)

## 2. `forward_over_stream`이 HTTP/1.1 연결을 구동한다

이 함수의 `IO`는 비동기 읽기와 쓰기가 가능한 스트림이다. 평문 경로는 TCP 스트림을 넘기고, 복호화 경로는 검증을 마친 업스트림 TLS 스트림을 넘긴다. Hyper의 `handshake`가 돌려준 `connection`은 별도 Tokio 작업에서 계속 구동하고, 원래 작업은 `sender`로 요청을 보낸다. [공통 전달 함수](../../../crates/http-inspector-core/src/interception/forward.rs#L45) · [복호화 경로](../../../crates/http-inspector-core/src/interception/inspect.rs)

요청을 보내기 직전 다음 코드가 실행된다.

```rust
	*request.uri_mut() = path;
	remove_hop_by_hop_headers(request.headers_mut());
	let request_body = BodyPreviewWriter::new(BODY_PREVIEW_BYTES);
	capture.writer.attach_request_body(request_body.preview());
```

`path`는 업스트림에 보낼 요청 대상이다. 예를 들어 클라이언트가 프록시에 보낸 `http://localhost:9000/items?q=a`는 이 단계에서 `/items?q=a`가 된다. 캡처에서 표시하는 대상과 헤더는 앞서 `CaptureContext::start`에서 읽어 두었다. `attach_request_body`는 미리보기와 전송 시작 시각을 공유 레코드에 연결한다. `ObservedBody`는 Hyper가 요청 본문의 프레임을 읽을 때 그 바이트를 관찰하고 같은 프레임을 전달한다. [요청 변환과 본문 연결](../../../crates/http-inspector-core/src/interception/forward.rs#L74) · [본문 관찰](../../../crates/http-inspector-core/src/interception/capture.rs)

## 3. 응답 헤더가 결과를 확정한다

| 분기 | 코드에서 하는 일 |
| --- | --- |
| Hyper HTTP 초기화 실패 | `FailureStage::Connect`로 기록하고 502를 만든다. |
| `send_request`에서 제한 시간 내 응답 헤더를 얻지 못함 | `FailureStage::ResponseTimeout`으로 기록하고 504를 만든다. 제한 시간은 응답 헤더를 얻을 때까지 적용된다. |
| `send_request`에서 HTTP 오류 반환 | `FailureStage::Response`로 기록하고 502를 만든다. |
| 응답 헤더 수신 | 헤더를 캡처하고 `Outcome::Response`를 확정한다. 전달용 응답에서 연결 단위 헤더를 제거한 뒤 본문을 `ObservedBody`로 감싸 반환한다. |

응답의 헤더 캡처가 전달용 헤더 정리보다 앞선다는 점에 유의한다. 산출물도 구분한다. 502와 504를 프록시가 만들어 보낸 분기는 레코드에 `Outcome::Failed`를 남긴다. 업스트림이 실제로 보낸 5xx 헤더는 `Outcome::Response`로 기록된다. 두 경로는 Requests 화면에서 서로 다른 결과로 분류된다. [응답 분기](../../../crates/http-inspector-core/src/interception/forward.rs#L81) · [실패 응답 생성](../../../crates/http-inspector-core/src/interception.rs)

헤더를 받은 뒤 본문이 중간에 끊기면 이미 확정된 상태 코드는 유지되고 본문의 수명 상태가 바뀐다. 이 상태의 소유권과 시간 계산은 [`record.rs` 해설](exchange-record.md)에 있다.

## 동작을 확인하는 테스트

```bash
cargo test --test proxy_behavior strips_fields_named_by_connection_in_both_directions
cargo test --test proxy_behavior records_a_timeout_when_upstream_never_responds
cargo test --test body_completion relays_each_chunk_before_the_body_ends
```

마지막 테스트는 서버가 전체 본문을 보내기 전에 첫 청크를 클라이언트에서 읽어 전달이 스트리밍되는지 확인한다. [헤더 정리 테스트](../../../tests/proxy_behavior.rs#L80) · [헤더 대기 테스트](../../../tests/proxy_behavior.rs#L163) · [본문 전달 테스트](../../../tests/body_completion.rs#L143)
