# UI와 분석

[개발자 가이드](README.md)

요청 목록과 URL 맵은 같은 캡처를 받는다. 목록은 최근 레코드의 화면 상태를 표현하고, 맵은 **Clear** 이후의 URL별 관찰 결과를 누적한다. 그래서 목록에서 오래된 행이 밀려나도 맵의 요청 수에는 포함될 수 있다.

## 프레임에 레코드가 들어오는 곳

`InspectorApp::render_ui`는 프레임을 그리기 전에 `receive_records`로 프록시 큐를 비운다. 레코드 한 건을 `RequestsState::record_added`와 `AnalysisPipeline::record_added`에 전달한다. 이어서 `AnalysisPipeline::update`로 진행 중인 레코드와 백그라운드 분석 결과를 반영한다. [프레임 갱신](../../src/ui/app/mod.rs)

`RequestsState`는 보관 중인 레코드, 검색과 필터, 선택한 레코드 ID를 관리한다. 목록의 보관 한도를 넘어서 선택한 레코드가 밀려나면 선택을 해제한다. `DetailState`는 상세 탭, 본문 보기와 이미지, cURL 복사와 재전송 상태를 관리한다. [요청 목록과 상세 상태](../../src/ui/app/state.rs)

## 목록 검색과 본문 표시

- `RequestsState::update_filters`가 목록에 남은 레코드를 검색 상태에 알려 준다. `RequestSearch`는 검색어와 옵션을 관리하고, `SearchWorker`는 레코드 메타데이터와 본문 미리보기를 백그라운드에서 검색한다. 본문 수신이 끝난 레코드는 검색에서 다시 검사할 수 있다. [요청 필터](../../src/ui/requests/filter.rs) · [검색 상태](../../src/ui/detail/search/controller.rs) · [검색 워커](../../crates/http-inspector-analysis/src/search/worker.rs)
- 필터 입력의 필드 조건과 자유 텍스트는 `RequestQuery`가 같은 규칙으로 해석하므로 명령줄 필터도 같은 레코드를 고른다. 결과 종류, 검색 일치 여부, URL 맵에서 선택한 범위가 화면에 표시할 행을 결정한다. 범위를 바꿔도 레코드는 보관 목록에 남는다. [목록 필터](../../crates/http-inspector-analysis/src/filter/mod.rs)
- 상세 화면에서는 `body_cache.rs`가 본문 스냅샷에서 표시할 데이터를 준비하고 `body_render.rs`가 egui 위젯을 그린다. 압축 해제, 형식 판정, 보기 방식별 정리와 안내 문구는 egui와 무관한 `http_inspector_content::view`가 맡으므로 화면과 명령줄이 같은 본문 결과를 낸다. 압축, 형식 감지, 포맷이나 이미지 디코딩 규칙이 문제라면 `http-inspector-content`를 확인한다. 검색 결과와 Pretty 보기의 강조가 다르면 목록 검색 텍스트와 화면에 정리한 텍스트를 각각 확인한다. [본문 준비](../../src/ui/detail/body_cache.rs) · [본문 렌더링](../../src/ui/detail/body_render.rs) · [본문 처리 크레이트](../../crates/http-inspector-content/src/)

## URL 맵과 엔드포인트 분석

1. `AnalysisPipeline::record_added`가 요청을 `UrlMap`의 노드에 배치한다. 엔드포인트 분석기는 `RecordAdded` 이벤트를 받아 노드와 요청 메서드별로 작업을 제출한다. [분석 파이프라인](../../crates/http-inspector-analysis/src/pipeline.rs) · [엔드포인트 작업 제출](../../crates/http-inspector-analysis/src/endpoint_analysis.rs)
2. 엔드포인트 워커는 진행 중인 레코드를 추적하고 종료된 레코드에서 요청·응답 구조를 추출한다. 완전한 본문이 필요하면 [`BodySource`](capture-and-storage.md#기능이-읽는-데이터)를 사용한다. 추적 한도를 넘으면 가장 오래된 작업부터 그 시점에 관찰한 데이터로 처리한다. [엔드포인트 워커](../../crates/http-inspector-analysis/src/endpoint_schema/analyzer.rs)
3. `AnalysisPipeline::update`가 URL 맵의 진행 상태를 갱신하고 추적하던 레코드가 끝났으면 `RecordSettled`를 보낸다. 같은 호출에서 완료된 분석 결과도 모은다. 결과가 반영되면 Map 상세 화면이 필드 이름, 유형과 등장 횟수를 표시한다. **Show in Requests**는 노드나 엔드포인트의 범위를 요청 목록의 필터로 넘긴다. [Map 상세 화면](../../src/ui/map/detail_pane.rs) · [목록 필터 연결](../../src/ui/requests/filter.rs)

목록에는 있는데 엔드포인트 구조가 비어 있으면 먼저 요청이 아직 진행 중인지 확인한다. 이후 URL 맵에서 노드를 찾았는지, 엔드포인트 작업 큐에 들어갔는지, 완전한 본문을 읽을 수 있었는지 확인한다. 실제 프록시를 통과한 요청이 집계되는 경로는 [엔드포인트 테스트](../../tests/endpoint_schema_integration.rs)에서 따라갈 수 있다.

## UI 번역 카탈로그 (`src/ui/i18n/`)

도메인 크레이트(`crates/*`)는 UI 언어에 의존하지 않으며, 화면에 표시하는 영어·한국어 문구와 도메인 오류의 현지화는 모두 [`src/ui/i18n/`](../../src/ui/i18n/mod.rs) 아래에 모은다.

### 모듈 구성

| 파일 | 역할 |
| --- | --- |
| [`shared.rs`](../../src/ui/i18n/shared.rs) | 여러 화면에서 공통으로 쓰는 라벨·버튼·상태 문구 (`Language::shared`) |
| [`toolbar.rs`](../../src/ui/i18n/toolbar.rs) | 상단 툴바, 설정 메뉴, 상태 바와 알림 (`Language::toolbar`) |
| [`requests.rs`](../../src/ui/i18n/requests.rs) | 요청 목록, 필터, 워터폴, 결과 상태 (`Language::requests`) |
| [`detail.rs`](../../src/ui/i18n/detail.rs) | 요청·응답 상세 탭, 본문·이미지·트리·검색 (`Language::detail`) |
| [`map.rs`](../../src/ui/i18n/map.rs) | URL 맵 트리, 관계 그래프, 범례, 엔드포인트 상세 (`Language::map`) |
| [`dialogs/`](../../src/ui/i18n/dialogs/mod.rs) | 대화상자·메뉴별 1:1 카탈로그(`browser`, `ca`, `copy_curl`, `har`, `scope_help`, `save`, `bulk_save`, `body_saving`, `range_merge`, `replay`, `hls`) |
| [`domain.rs`](../../src/ui/i18n/domain.rs) | `crates/*`의 도메인 오류·열거형을 사용자 문구로 변환 |
| [`fmt.rs`](../../src/ui/i18n/fmt.rs) | 언어·테마 설정 저장 오류(`SettingsSaveError`) 현지화 |

### 문구 추가 규칙

1. **정적 문구 (`&'static str`, 0-allocation)**: 해당 화면 카탈로그의 `define_catalog!` 블록에 필드를 추가하고 영어(`en`)와 한국어(`ko`) 상수 테이블을 함께 채운다. 화면 코드는 `lang.<screen>()`이 돌려주는 `&'static <Screen>Text` 필드를 직접 읽는다.
2. **툴팁 분리**: 호버 툴팁은 같은 `define_catalog!`의 `tooltips: pub(crate) struct <Screen>Tooltips` 블록에 분리해 적는다. 툴팁은 [디자인 시스템](../ui-design-system.md) `D14`와 `S1`(한 줄, 한 문장 이내) 규칙을 지킨다.
3. **동적 포맷 문구**: `format!` 인자가 필요한 문구는 해당 화면 카탈로그 모듈에 타입 안전한 함수(예: `i18n::toolbar::request_count`, `i18n::dialogs::hls::live_segments_summary`)로 둔다.
4. **상태에 보관하는 오류**: 백그라운드 작업이나 설정 저장처럼 상태 필드에 오류를 저장했다가 나중에 렌더링하는 경우, 번역된 `String`을 저장하지 않고 구조화 `enum`(예: `SessionError`, `LimitSaveError`, `SettingsSaveError`, `BodySaveFailure`, `HlsInspectFailure`)을 저장한 뒤 렌더링 시점에 `i18n` 함수로 변환해 언어 전환 시 즉시 재번역되도록 한다.

### 경계 강제와 정책 테스트

- **컴파일 타임 차단**: `Language::pick`은 `src/ui/i18n/` 모듈 내부 전용(`const fn pick`)이므로 바깥 화면 코드에서 호출할 수 없다.
- **정적 경계 검사**: [`scripts/check-boundaries.py`](../../scripts/check-boundaries.py)가 `src/ui/i18n/` 바깥의 비테스트 코드에서 `.pick(...)` 호출과 `Language::English`/`Language::Korean` 직접 분기·임포트를 차단한다.
- **문구 정책 테스트**: [`src/ui/i18n/tests.rs`](../../src/ui/i18n/tests.rs)는 `define_catalog!`가 생성한 `entries()`로 모든 정적 카탈로그 필드가 비어 있지 않음을 확인하고, 모든 정적 툴팁과 동적 툴팁 함수가 `S1`(한 문장 이내) 규칙을 만족하는지 자동 검증한다. 새 카탈로그를 추가하면 `tests.rs`의 `all_static_entries`와 `all_static_tooltips`에도 등록한다.
