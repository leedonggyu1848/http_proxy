# HTTP Inspector 코드베이스 가이드

HTTP Inspector에 처음 기여하는 개발자를 위한 가이드다. 로컬 HTTP 요청 한 건을 따라가면서 네트워크 전달, 캡처, 화면 표시가 어디서 이어지는지 살펴본다. 환경 설정과 전체 검증 명령은 [개발 문서](../development.md)에 있다.

## 처음 읽을 순서

1. [요청의 수명](request-lifecycle.md)에서 프록시가 언제 요청 행을 만들고 언제 응답과 본문을 기록하는지 확인한다.
2. [캡처와 저장](capture-and-storage.md)에서 화면, 로그, 내보내기가 서로 다른 데이터를 보여 줄 수 있는 이유를 확인한다.
3. [UI와 분석](ui-and-analysis.md)에서 같은 요청이 목록, 검색 결과, URL 맵에 반영되는 흐름을 살펴본다.
4. [테스트 선택](testing.md)에서 수정하려는 동작을 다루는 테스트를 찾는다.

## 코드의 책임

| 위치 | 맡는 일 |
| --- | --- |
| [앱 바이너리](../../src/) | eframe 시작, 프록시와 분석기 연결, 화면 상태와 사용자 동작 |
| [`http-inspector-core`](../../crates/http-inspector-core/) | 프록시 런타임, HTTPS와 브라우저 연동, 재전송, 세션 로그와 본문 저장 |
| [`http-inspector-exchange`](../../crates/http-inspector-exchange/) | 공유 캡처 타입, 헤더 가림, 미리보기 한도와 상태 |
| [`http-inspector-content`](../../crates/http-inspector-content/) | 압축 해제, 본문 형식 감지와 정리, 이미지 디코딩 |
| [`http-inspector-analysis`](../../crates/http-inspector-analysis/) | 레코드 목록과 필터, 검색, URL 맵과 엔드포인트 집계 |
| [`http-inspector-interop`](../../crates/http-inspector-interop/) | HAR, cURL 명령, URL 맵의 내보내기 형식 |

허용되는 크레이트 의존 방향과 캡처 쓰기 핸들의 소유권은 [워크스페이스 구조와 의존 규칙](../development.md#워크스페이스-구조와-의존-규칙)에 정리되어 있다.

## 증상별로 처음 볼 곳

| 증상 | 코드에서 시작할 곳 |
| --- | --- |
| 브라우저가 요청을 보냈지만 전달에 실패한다 | [`proxy_request`가 있는 프록시 진입점](../../crates/http-inspector-core/src/interception/handle.rs)과 [업스트림 연결](../../crates/http-inspector-core/src/interception/upstream.rs) |
| 상태 코드는 있지만 본문이 끝나지 않았다 | [`Exchange::is_live`](../../crates/http-inspector-exchange/src/exchange/record.rs)와 [`ObservedBody`](../../crates/http-inspector-core/src/interception/capture.rs) |
| 화면에 보이는 본문의 뒷부분을 검색할 수 없다 | [검색 문서 생성과 본문 범위](../../crates/http-inspector-analysis/src/search/document.rs) |
| 요청 행은 있지만 엔드포인트 구조가 아직 비어 있다 | [`AnalysisPipeline::update`](../../crates/http-inspector-analysis/src/pipeline.rs)와 [본문 분석 작업](../../crates/http-inspector-analysis/src/endpoint_analysis.rs) |
| **Clear** 뒤에도 전체 로그에서 요청을 찾을 수 있다 | [`InspectorApp::clear_session`](../../src/ui/app/mod.rs)과 [세션 저널](../../crates/http-inspector-core/src/recording/journal.rs) |
