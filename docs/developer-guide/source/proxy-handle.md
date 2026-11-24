# `handle.rs`에서 프록시 시작과 종료 따라가기

[개발자 가이드](../README.md) · [`handle.rs` 열기](../../../crates/http-inspector-core/src/interception/handle.rs)

**Start**를 눌렀는데 포트 오류가 나거나 **Stop** 뒤 행이 미완료로 표시되면 이 파일부터 읽는다. `ProxyHandle`은 앱에 시작·종료와 캡처 수신 API를 제공한다. `Engine`은 한 번의 프록시 실행에서 여러 연결이 공유하는 상태다.

## 1. `start_inner`가 실행 경계를 만든다

`start`와 `start_recording`은 세션 전체 로그 유무를 인자로 받아 같은 `start_inner`로 들어간다. 이 함수는 다음 순서로 준비한다. [시작 함수](../../../crates/http-inspector-core/src/interception/handle.rs#L71)

1. 복호화 설정이 있으면 `TlsInspector::open`을 호출한다. 이 단계가 성공하면 엔진에서 쓰는 TLS 상태와 핸들이 반환할 공개 CA 정보를 얻는다.
2. 표준 TCP 리스너를 바인드하고 논블로킹으로 바꾼다. 실제 주소는 `listener.local_addr()`에서 읽는다. 포트 0을 요청한 경우에도 호출자는 `ProxyHandle::local_addr()`에서 배정된 포트를 알 수 있다.
3. 멀티스레드 Tokio 런타임, 한도가 있는 캡처 채널, 종료 신호용 `watch` 채널을 만든다. `Engine`에는 이벤트 송신측과 연결 제한용 세마포어를 넣는다. `ProxyHandle`에는 수신측, 종료 송신측과 스레드 핸들을 넣는다.
4. 이름이 `http-inspector-proxy`인 스레드가 런타임에서 `run`을 실행한다. `SessionId::next()`는 프록시를 시작할 때마다 새 ID를 만든다. 전체 로그나 본문 저장소는 호출자가 넘긴 인스턴스를 재사용할 수 있다.

초기 오류는 `StartError::Bind`, `StartError::Https`처럼 실패 단계에 맞는 값으로 반환된다. 시작 후 리스너가 치명적인 오류로 멈추면 스레드는 오류를 `EngineStatus`에 기록한다. UI는 `ProxyHandle::state()`로 이를 읽는다. [엔진 상태](../../../crates/http-inspector-core/src/interception/status.rs)

## 2. `run`에서 리스너와 연결 슬롯을 읽는다

`run`은 종료 신호와 `listener.accept()`를 `select!`로 기다린다. 연결을 수락한 다음 `acquire_owned()`로 슬롯을 얻는다. 슬롯이 모두 사용 중이면 이미 수락한 소켓은 대기하고, 그동안 루프는 다음 `accept()`로 진행하지 않는다. 슬롯을 기다리는 동안에도 종료 신호를 받을 수 있다. 일시적인 수락 오류는 짧게 기다린 뒤 재시도하고, 치명적인 오류는 반환한다. [수락 루프](../../../crates/http-inspector-core/src/interception/handle.rs#L249)

슬롯을 얻으면 `serve_connection`을 별도 Tokio 작업으로 실행한다. 이 함수는 헤더 읽기 제한 시간이 있는 Hyper HTTP/1.1 서버를 만들고 `with_upgrades()`를 활성화한다. 각 요청은 같은 연결 슬롯의 `Arc`를 받아 `proxy_request`에서 처리된다. CONNECT가 업그레이드되면 그 작업이 슬롯을 이어서 보유하므로 터널을 열었다고 연결 한도가 풀리지 않는다. [연결 처리](../../../crates/http-inspector-core/src/interception/handle.rs#L315) · [CONNECT 업그레이드](../../../crates/http-inspector-core/src/interception/inspect.rs)

## 3. `proxy_request`가 캡처한 뒤 전달 방식을 고른다

두 분기 모두 먼저 `CaptureContext::start`를 호출한다. CONNECT는 `CaptureSource::Connect`로 기록하고 `connect_tunnel`에 넘긴다. 다른 HTTP 메서드는 `CaptureSource::HttpProxy`로 기록하고 `forward_http`를 호출한다. 이 순서 때문에 목적지 연결이 실패해도 이미 게시된 요청에 실패 이유를 채울 수 있다. [요청 분기](../../../crates/http-inspector-core/src/interception/handle.rs#L334) · [전달 함수 해설](http-forwarding.md)

이벤트는 `try_send`로 게시한다. 큐가 가득 차면 누락 건수를 늘린다. 성공한 게시만 콜백을 호출하며, 화면은 `try_receive`로 대기 없이 읽는다. 게시한 `Exchange`에는 공유 상태가 있으므로 결과를 별도 이벤트로 보낼 필요가 없다. [이벤트 게시 코드](../../../crates/http-inspector-core/src/interception/capture_context.rs#L27)

## 4. 종료 순서가 요청의 마지막 상태를 결정한다

`stop_inner`는 `watch`에 종료 신호를 쓰고 `JoinHandle`을 꺼내 스레드 종료를 기다린다. `Drop`도 같은 정리 함수를 쓴다. 수락 루프가 빠져나오면 스레드는 `stopped`를 먼저 설정하고 Tokio 런타임을 내린다. 그때 결과를 확정하지 못한 `CaptureContext`가 해제되면 `Incomplete`를 기록한다. 런타임이 실행 중에 같은 컨텍스트가 결과 없이 해제되면 `Cancelled`를 기록한다. [종료 함수](../../../crates/http-inspector-core/src/interception/handle.rs#L208) · [캡처 종료 상태](../../../crates/http-inspector-core/src/interception/capture_context.rs#L136)

순서를 결정하는 코드는 스레드가 `run`을 마친 직후에 있다.

```rust
	stopped.store(true, Ordering::Release);
	drop(runtime);
```

## 동작을 확인하는 테스트

```bash
cargo test --test proxy_behavior holds_new_clients_while_the_connection_limit_is_reached
cargo test --test proxy_edge_cases stop_closes_and_marks_pending_exchanges_and_open_tunnels_incomplete
```

첫 테스트는 슬롯이 사용 중일 때 새 요청이 기다리는 동작을 확인한다. 두 번째는 종료하면 대기 요청과 열린 터널이 미완료로 기록되는지 확인한다. [연결 한도 테스트](../../../tests/proxy_behavior.rs#L201) · [종료 테스트](../../../tests/proxy_edge_cases.rs#L443)
