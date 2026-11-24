# `pipeline.rs`에서 캡처 이벤트와 분석 결과 따라가기

[개발자 가이드](../README.md) · [`pipeline.rs` 열기](../../../crates/http-inspector-analysis/src/pipeline.rs)

Map에 경로가 나타났는데 엔드포인트 본문 구조는 나중에 표시될 수 있다. URL 맵 배치는 레코드가 들어올 때 이 파일에서 처리한다. 본문 구조는 워커가 추출하고 결과가 준비된 프레임에서 합친다.

## 1. `AnalysisPipeline`을 구성하는 필드

| 필드 | 역할 |
| --- | --- |
| `url_map` | 요청을 노드에 배치하고 경로와 관계 집계를 제공한다. |
| `endpoints` | 기본 엔드포인트 분석기를 보유한다. |
| `analyzers` | `register`로 추가한 분석기를 등록 순서대로 보유한다. 새 분석기는 이후 이벤트부터 받는다. |
| `watched`, `unwatched` | 진행 중인 레코드 중 종료 이벤트를 보낼 대상과 추적 한도 때문에 놓친 수를 기록한다. |
| `wake` | 백그라운드 결과가 준비되면 UI에 프레임을 요청하는 콜백이다. |

운영 경로인 `start`는 `BodySource`와 `wake`로 엔드포인트 분석 워커를 시작한다. `disabled`와 `seeded`는 백그라운드 작업 없이 URL 맵을 구성한다. `seeded`는 이미 채운 URL 맵을 받으므로 합성 데이터로 UI를 확인하는 테스트에 쓸 수 있다. [필드와 초기화](../../../crates/http-inspector-analysis/src/pipeline.rs#L117)

## 2. `record_added`가 노드를 정한 뒤 이벤트를 보낸다

```rust
	let node = self.url_map.observe_exchange(exchange);
	self.dispatch(&CaptureEvent::RecordAdded { exchange, node });
```

`NodeId`는 모든 분석기에 같은 배치 결과를 전달한다. 배치할 수 없는 대상이면 `node`는 `None`이다. `dispatch`는 먼저 기본 엔드포인트 분석기에 이벤트를 보내고, 이어서 추가 분석기를 등록 순서대로 호출한다. [노드 배치와 게시](../../../crates/http-inspector-analysis/src/pipeline.rs#L195) · [호출 순서](../../../crates/http-inspector-analysis/src/pipeline.rs#L274)

| `RecordAdded` 직후 상태 | `RecordSettled` 처리 |
| --- | --- |
| 이미 `is_live() == false` | 같은 호출에서 바로 이벤트를 보낸다. |
| 진행 중이고 추적 공간이 있음 | 노드 ID와 읽기 핸들을 저장한다. 나중에 `update`가 종료를 발견하면 한 번 보낸다. |
| 진행 중이고 추적 공간이 없음 | `unwatched`를 늘린다. 이 레코드는 나중에 끝나도 종료 이벤트를 받지 않는다. |

이 파일에서 종료를 뜻하는 `RecordSettled`는 `Exchange::settled().await`가 돌아오는 시점과 다를 수 있다. 예를 들어 응답 헤더를 받았더라도 본문이 스트리밍 중이면 `is_live()`가 참이므로 파이프라인은 종료 이벤트를 미룬다. [공유 레코드의 진행 상태](exchange-record.md#2-settle-settled-is_live를-구분한다)

## 3. 엔드포인트 분석은 추가 이벤트에서 시작한다

`EndpointAnalysis::handle`은 노드가 있는 `RecordAdded`에서 작업을 제출한다. 워커는 이때 받은 `Exchange`의 진행 상태를 자체적으로 확인한다. 그래서 파이프라인의 종료 추적 한도와 엔드포인트 작업 큐 한도는 서로 다른 제약이다. 경로가 없는 CONNECT 레코드는 작업에서 제외된다. [기본 분석기의 이벤트 처리](../../../crates/http-inspector-analysis/src/pipeline.rs#L92) · [작업 제출](../../../crates/http-inspector-analysis/src/endpoint_analysis.rs#L45) · [엔드포인트 워커](../../../crates/http-inspector-analysis/src/endpoint_schema/analyzer.rs)

## 4. `update`가 상태를 반영하고 `session_cleared`가 집계를 비운다

매 프레임 `update`는 URL 맵의 진행 데이터를 동기화하고, 추적 레코드 중 끝난 레코드의 종료 이벤트를 보낸다. 이어서 내장 분석기와 추가 분석기의 `poll`을 호출해 준비된 결과를 가져온다. `true`는 이 단계 중 하나가 변화를 보고했거나 종료 이벤트를 보냈다는 뜻이다. 종료 이벤트가 발생해도 화면의 집계 값은 그대로일 수 있다. [프레임 갱신](../../../crates/http-inspector-analysis/src/pipeline.rs#L224)

`session_cleared`는 맵, 추적 목록과 누락 카운터를 비우고 모든 분석기에 `SessionCleared`를 보낸다. 엔드포인트 워커는 작업에 붙인 세대 번호로 지우기 전의 결과를 구분한다. 이미 처리 중이던 결과가 늦게 도착해도 현재 집계에 다시 합치지 않는다. [집계 초기화](../../../crates/http-inspector-analysis/src/pipeline.rs#L211) · [워커 초기화와 결과 수집](../../../crates/http-inspector-analysis/src/endpoint_schema/analyzer.rs#L119)

## 동작을 확인하는 테스트

```bash
cargo test -p http-inspector-analysis records_beyond_the_watch_limit_get_no_settled_event
cargo test -p http-inspector-analysis clear_resets_url_map_endpoint_schemas_and_every_analyzer
cargo test --test endpoint_schema_integration live_traffic_builds_endpoint_structure_without_values
```

앞의 두 테스트는 종료 이벤트 한도와 초기화를 확인한다. 마지막 테스트는 실제 로컬 요청이 엔드포인트 구조로 집계되는 경로를 확인한다. [파이프라인 단위 테스트](../../../crates/http-inspector-analysis/src/pipeline.rs#L407) · [엔드포인트 통합 테스트](../../../tests/endpoint_schema_integration.rs#L203)
