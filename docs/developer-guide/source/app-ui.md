# `app/mod.rs`에서 한 프레임 따라가기

[개발자 가이드](../README.md) · [`app/mod.rs` 열기](../../../src/ui/app/mod.rs)

`InspectorApp`은 프록시 이벤트의 수신, 백그라운드 작업 결과 반영, 화면 그리기를 한 프레임 안에서 연결한다. 증상이 목록과 URL 맵에서 다르게 보이거나, 새 요청이 도착했는데 화면이 늦게 바뀌면 이 파일에서 호출 순서를 먼저 확인한다.

## 1. `new`가 앱 전체 상태를 연결한다

`main`은 로깅과 창 옵션을 설정하고 eframe 시작 콜백에서 글꼴과 `InspectorApp`을 초기화한다. `InspectorApp::new`는 먼저 `SessionState`를 만든다. 그 세션의 `BodySource`와 다시 그리기 콜백을 분석 파이프라인에 넘긴 뒤 프록시를 시작한다. [앱 진입점](../../../src/ui/app/mod.rs#L36) · [앱 초기화](../../../src/ui/app/mod.rs#L75)

`InspectorApp`의 `session`은 프록시와 세션 저장소를, `requests`는 목록·필터·선택을, `detail`은 본문 표시와 재전송을 맡는다. `map`은 분석 파이프라인과 맵 패널을 담는다. 저장 대화상자와 창 배치는 각각 `dialogs`, `layout`을 통해 읽는다. [상태 정의](../../../src/ui/app/state.rs)

## 2. `render_ui`는 데이터를 읽은 뒤 화면을 그린다

1. `receive_records`가 `ProxyHandle::try_receive`로 그때 읽을 수 있는 레코드를 꺼낸다. 각 `Exchange`를 맵 분석기에 먼저 빌려 주고 요청 목록으로 이동시킨다. 목록 보관 한도로 행이 밀려나도 앞서 분석기에 넘긴 데이터는 이미 집계에 쓰일 수 있다. [레코드 수신](../../../src/ui/app/mod.rs#L129)
2. `AnalysisPipeline::update`로 맵 진행과 완료된 분석 결과를 반영한다. 이어서 엔진이 자체 오류로 멈췄는지 확인하고, 본문 저장·재전송·HAR 같은 UI 작업의 상태를 갱신한다. [프레임 진입점](../../../src/ui/app/mod.rs#L157)
3. 스크롤 영역 안에서 상단 바, 알림, 선택한 보기, 상태 바를 그린다. 상단 바(`requests/top_bar.rs`)는 프록시 상태, `Requests | Map` 전환, HTTPS 상태와 주요 동작의 세 구역을 이전 프레임에서 잰 크기로 배치하고, 크기가 바뀐 프레임은 `request_discard`로 다시 그린다. 보기 전환은 `MainView`를 바꾼다. Requests는 필터와 목록·상세를, Map은 설정과 트리 또는 그래프를 표시한다. 현재 프레임에서 Map이 반환한 `MapAction`은 스크롤 영역을 그린 뒤 처리한다. [보기 전환과 작업 적용](../../../src/ui/app/mod.rs#L184) · [상단 바 배치](../../../src/ui/requests/top_bar.rs)

레코드 한 건을 두 곳에 넘기는 코드는 `receive_records`에 있다.

```rust
	while let Some(record) = proxy.try_receive() {
		self.map.analysis.record_added(&record);
		self.requests.record_added(record);
	}
```

Map의 **Show in Requests**는 경로 범위 필터를 설정한 뒤 Requests로 전환한다. `SelectRequest`는 레코드 ID를 선택하고 Requests로 전환한다. HAR 창은 확인 시점에 전체 또는 필터된 목록에서 `Exchange` 읽기 핸들을 복제해 저장 작업에 넘긴다. 이때 복제한 핸들은 [공유 레코드](exchange-record.md)와 같은 상태를 계속 읽는다. [Map 작업](../../../src/ui/app/mod.rs#L239) · [HAR 내보내기](../../../src/ui/app/mod.rs#L285)

## 3. 다시 그리는 이유를 확인한다

| 변화 | 프레임을 요청하는 곳 |
| --- | --- |
| 새 요청 게시 | 프록시의 이벤트 콜백 |
| 엔드포인트 분석 결과 도착 | 파이프라인을 시작할 때 전달한 콜백 |
| 목록에 진행 중인 레코드가 있음 | `LIVE_REPAINT` 간격으로 예약해 공유 본문·터널 상태를 다시 읽음 |
| 목록에 진행 중인 레코드가 없고 프록시 핸들이 있음 | `IDLE_REPAINT` 간격으로 예약해 엔진의 자체 종료를 확인 |

수신 큐에서 새 레코드를 가져오는 시점과 이미 화면에 있는 레코드의 바이트 수가 바뀌는 시점은 다르다. 기존 행의 표시만 늦다면 재그리기 예약과 `requests.has_live_records()`부터 확인한다. [다시 그리기 예약](../../../src/ui/app/mod.rs#L212) · [프록시 시작 알림 등록](../../../src/ui/app/state.rs#L71)

## 4. 중지, 지우기, 패널 배치가 각각 바꾸는 상태

- **엔진 자체 종료.** `release_failed_proxy`는 실패 상태를 발견하면 수신 큐를 한 번 더 읽고, `take_proxy`로 핸들을 분리한다. 이때 누락 건수를 목록 상태에 넘겨 상태 바의 집계에 반영한다. 이어서 종료를 기다리고 오류 메시지를 남긴다. [엔진 오류 처리](../../../src/ui/app/mod.rs#L106)
- **Clear.** `clear_session`은 현재 엔진 누락 건수를 기준으로 목록·선택·맵 필터를 초기화하고 분석을 비운다. 세션 로그와 본문 파일의 수명은 `SessionState`에 있다. [화면 기록 초기화](../../../src/ui/app/mod.rs#L139) · [캡처와 저장](../capture-and-storage.md)
- **패널 너비.** Requests와 Map 트리는 `split_panes`와 같은 `detail_share`를 쓴다. 이 값은 선호 비율이며, 실제 오른쪽 너비는 매 프레임 창 크기에 맞게 제한한다. 두 패널의 클리핑 범위를 각각 정해 한쪽의 넓은 내용이 반대쪽에 그려지지 않게 한다. 핸들을 끌 때 선호 비율을 갱신한다. [패널 분할](../../../src/ui/app/mod.rs#L386)

## 동작을 확인하는 테스트

```bash
cargo test -p http-proxy clear_drops_capture_state_and_keeps_view_preferences
cargo test -p http-proxy split_divider_drag_repaints_and_restores_across_resize
```

첫 테스트는 Clear가 목록과 맵 집계를 비우고 표시 설정을 유지하는지 확인한다. 두 번째는 분할 경계를 끌고 창을 좁혔다 넓힌 뒤 선호 너비가 다시 적용되는지 확인한다. [Clear 테스트](../../../src/ui/app/mod.rs#L556) · [창 크기 변경 테스트](../../../src/ui/app/mod.rs#L928)
