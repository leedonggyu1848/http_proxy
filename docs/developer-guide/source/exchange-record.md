# `record.rs`에서 공유 레코드와 시간 읽기

[개발자 가이드](../README.md) · [`record.rs` 열기](../../../crates/http-inspector-exchange/src/exchange/record.rs)

Requests 화면에 200이 보이면서 다운로드 바이트 수가 계속 늘 수 있다. 이 파일에는 응답 결과가 확정되는 시점과 데이터가 더 들어올 수 있는지를 따로 읽는 코드가 있다.

## 1. 하나의 상태를 엔진이 쓰고 여러 곳에서 읽는다

`ExchangeWriter::start`가 `Arc<ExchangeInner>`를 만든다. `RecordId`, 프록시 실행을 나타내는 `SessionId`, 캡처 경로인 `CaptureSource`, 캡처한 요청 헤더는 이때 정해진다. `replay_of`, 연결 시간, 송신 시작 시점과 결과는 각 `OnceLock`에 나중에 채운다. [내부 상태와 생성자](../../../crates/http-inspector-exchange/src/exchange/record.rs#L328)

`Exchange`를 복제하면 `Arc`가 공유된다. UI가 받은 복제본은 엔진이 이후에 채운 결과와 본문 진행 상태를 볼 수 있다. 쓰기 권한은 캡처 경로의 `ExchangeWriter`에 있다. `attach_request_body`는 요청 본문 미리보기를 연결하면서 `send_started`를 한 번 기록한다. [쓰기 핸들](../../../crates/http-inspector-exchange/src/exchange/record.rs#L363) · [본문 미리보기 연결](../../../crates/http-inspector-exchange/src/exchange/record.rs#L418)

## 2. `settle`, `settled`, `is_live`를 구분한다

- **`outcome()`과 `settle(outcome)`.** 결과 슬롯이 비어 있으면 `outcome`은 `InFlight`를 반환한다. `settle`은 결과와 경과 시간을 함께 저장한다. 첫 번째 쓰기가 성공하면 대기 중인 작업을 깨운다. 이후 쓰기는 `false`를 반환하며 처음 기록한 값을 유지한다. [결과 확정](../../../crates/http-inspector-exchange/src/exchange/record.rs#L435) · [결과 조회](../../../crates/http-inspector-exchange/src/exchange/record.rs#L486)
- **`settled().await`.** 결과가 있으면 바로 반환한다. 결과가 없으면 아래 코드처럼 현재 작업의 waker를 등록한다. HTTP 응답에서는 헤더가 도착하면 이 대기가 끝난다. [비동기 대기](../../../crates/http-inspector-exchange/src/exchange/record.rs#L565)
- **`is_live()`.** `InFlight`에서는 참이다. `Response`에서는 응답 본문이 `Streaming`인 동안, `Tunnel`에서는 터널 상태가 `Open`인 동안 참이다. UI의 재그리기와 URL 맵의 진행 추적은 이 값을 쓴다. 이 함수의 응답 분기는 응답 본문의 수명을 본다. 요청 본문까지 읽는 검색은 검색 데이터가 더 바뀔지를 자체 기준으로 판단한다. [진행 상태](../../../crates/http-inspector-exchange/src/exchange/record.rs#L495) · [검색의 종료 판단](../../../crates/http-inspector-analysis/src/search/document.rs#L96)

`settled()`가 대기자를 등록하는 부분이다. 등록 후 결과를 다시 읽으므로, 첫 조회 뒤에 이미 끝난 작업도 반환할 수 있다.

```rust
	if let Some(outcome) = outcome() {
		return Poll::Ready(outcome);
	}
	self.inner.settled_wakers.register(cx.waker());
	// Checked again so a settlement between the check and the
	// registration still completes this wait.
	outcome().map_or(Poll::Pending, Poll::Ready)
```

예를 들어 응답 헤더 직후에는 `outcome()`이 `Response`이고 `settled().await`도 반환할 수 있다. 응답 본문이 계속 도착하면 `is_live()`는 참이며 `received_bytes()`는 이후 조회에서 늘어난 값을 반환할 수 있다.

## 3. `timing`이 관찰한 시점으로 구간을 만든다

`started_at`은 화면과 내보내기에서 쓰는 벽시계 시각이다. 경과 시간은 `Instant`를 기준으로 계산한다. `timing()`은 다음 값을 필요할 때 조합한다. [시간 계산](../../../crates/http-inspector-exchange/src/exchange/record.rs#L523)

| 필드 | 시작과 끝 |
| --- | --- |
| `connect` | 연결 코드가 기록한 업스트림 연결 소요 시간 |
| `send` | 요청 전송 시작부터 요청 본문 종료까지 |
| `wait` | 요청 본문 종료부터 응답 헤더 도착까지 |
| `receive` | 응답 헤더 도착부터 응답 본문 종료까지 |
| `settled` | 레코드 시작부터 결과 확정까지 |
| `complete` | HTTP 응답은 레코드 시작부터 응답 본문 종료까지. 그 외 결과는 `settled`와 같다. |

`between`은 양 끝을 확인하고 `checked_sub`로 뺀다. 서버가 요청 본문이 끝나기 전에 응답하면 `wait`는 `None`이다. `elapsed()`는 결과가 정해지면 `settled`를 반환한다. 열린 터널의 남은 수명은 `TunnelPreview`에서 확인한다. [경과 시간과 구간 계산](../../../crates/http-inspector-exchange/src/exchange/record.rs#L558)

## 동작을 확인하는 테스트

```bash
cargo test -p http-inspector-exchange clones_share_progress
cargo test -p http-inspector-exchange leaves_wait_unobserved_when_the_response_beats_the_request_body
cargo test -p http-inspector-exchange wakes_a_waiter_when_settled
```

복제본이 엔진과 상태를 공유하는지, 헤더가 요청 본문 종료보다 먼저 오면 `wait`가 비어 있는지, 결과를 확정하면 등록된 대기자가 깨어나는지를 각각 확인한다. [레코드 단위 테스트](../../../crates/http-inspector-exchange/src/exchange/record.rs#L740)
