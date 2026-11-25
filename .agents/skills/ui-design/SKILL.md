---
name: ui-design
description: "HTTP Inspector의 UI 디자인 정책. 화면 설명 글을 없애는 원칙, 정밀 도킹 워크벤치(Slate Navy & Muted Teal) 디자인 시스템의 색, 모양, 글, 배치 규칙과 PR 전 캡처 점검 절차를 정한다. src/ui의 화면, 위젯, 스타일, 화면 문구를 추가하거나 바꿀 때와 UI를 바꾼 PR을 리뷰할 때 사용한다."
---

# UI 디자인 정책

HTTP Inspector는 개발 도구다. 디자인 시스템은 정밀 도킹 워크벤치이고 테마는 Slate Navy & Muted Teal이다. 화면을 1px 선으로 나눈 평면 패널로 채우고, 면적의 95% 이상을 무채색으로 둔다. 빽빽함과 글자 중심의 기준은 DevTools Network 패널, Wireshark, mitmproxy, Charles다.

가장 먼저 설명 글을 없앤다. 라벨, 값, 컨트롤 상태로 이해되는 화면에는 설명 문장을 쓰지 않는다. 이해되지 않으면 문장을 더하지 말고 라벨과 배치를 고친다. 색과 상자도 지웠을 때 잃는 정보를 말할 수 있을 때만 남긴다.

규칙 번호는 리뷰에서 인용하는 고정 식별자다. 삭제된 규칙의 번호는 빈 채로 두고, 새 규칙은 다음 번호를 받아 해당 절 끝에 둔다. 각 규칙은 캡처나 코드를 보고 예나 아니오로 판정한다.

색 값, 글자 크기, 치수, 컴포넌트 규격과 그 Rust 이름은 [design-system.md](design-system.md)에 있다. 스타일 토큰이나 위젯을 만들거나 바꿀 때, 그리고 리뷰에서 값을 대조할 때 읽는다.

현재 코드의 다음 항목은 새 시스템과 다른 정리 대상이다. 코드에 있다는 사실은 다시 쓸 근거가 아니다.

- `src/ui/style/tokens.rs`의 `LIGHT`·`DARK` 색 값 전체, `ToneFills`, `ToneColors::fill`, `Tone::Neutral`, `Tone::Info`
- `Radius::CONTROL`(6pt), `Radius::BADGE`, `Radius::CARD`, `TypeScale`의 현재 값
- `src/ui/style/badge.rs`의 `Badge`와 `BADGE_HEIGHT`
- `src/ui/requests/request_table.rs`의 `ALERT_EDGE_WIDTH`(3pt 띠)와 pill의 이중 표시, 지역 상수 `ROW_HEIGHT`·`HEADER_HEIGHT`(24pt), `.striped(true)`
- `src/ui/style/theme.rs`에서 선택 글자를 `accent`로 칠하는 `selection.stroke`, `hyperlink_color = info`, egui 기본 그림자와 `animation_time`(0.2초)

## 색

1. 정상 상태는 기본 글자색으로 쓴다. 2xx 응답 행, `GET`·`POST` 같은 일반 메서드, 완료된 단계에 의미 색을 칠하지 않는다. `status.ok`는 연결됨, 실행 중 같은 상태 도트에만 쓴다.
2. 의미 색은 예외와 상태 변화에만 쓴다. 4xx·5xx 응답, 연결 실패, 취소, 프록시 중지가 여기에 해당한다.
3. `accent.primary`는 선택 바, 켜진 토글 바, 활성 경로, 차트 선에 쓴다. `accent.secondary`는 포커스 링, 교차 강조, 스플리터 호버에 쓴다. Primary 버튼은 화면에 하나 이하다.
4. 같은 정보를 두 가지 표시로 반복하지 않는다. 실패 행이라면 레프트 틱과 상태 글자색 중 하나만 쓴다. 선택 행과 켜진 토글의 배경과 2px 바 조합(33, 35)은 한 표시로 센다. 선택은 실패와 다른 정보이므로 함께 써도 되고, 행 왼쪽 끝은 선택 바가 쓰며 레프트 틱은 상태 셀의 글자 앞에 둔다.
5. 그라데이션, 글로우, 배경 블러, 의미 없는 장식 색을 쓰지 않는다. 그림자는 팝업과 드롭다운 메뉴의 경계 그림자만 허용한다. 반투명 색은 `surfaces.hover`, `surfaces.selected`, `surfaces.cross_highlight` 토큰으로만 쓴다.
6. 색 값은 `src/ui/style/tokens.rs`에만 정의한다. 화면 코드는 토큰 이름이나 `Tone`으로 의미를 고르고 `Color32::from_rgb`·`from_rgba_unmultiplied`를 직접 쓰지 않는다.
7. 색만으로 의미를 전달하지 않는다. 상태는 `404 Not Found`처럼 글자로도 보인다. 글자와 그 글자가 실제로 놓이는 배경의 대비는 라이트·다크 테마 모두 WCAG AA(4.5:1) 이상이다. 도트, 틱, 바, 포커스 링은 인접 배경과 3:1 이상이다. `src/ui/style/theme.rs`의 `status_text_meets_aa_on_all_widget_backgrounds`가 글자 대비를 검사한다. 새 글자색은 그 테스트의 글자색 목록에, 글자가 놓이는 새 배경은 `text_backgrounds()`에 추가한다.

### 대비와 범주 색

25. 다음 토큰은 4.5:1에 못 미치는 배경이 있으므로 정한 용도에만 쓴다. 토큰 값을 바꾸지 않고 용도를 줄인다. 계산값은 design-system.md의 "대비"에 있다.
    - `text.muted`: 비활성 아이콘, 키-값 콜론, 줄 번호·오프셋 거터, 라벨이나 툴팁이 따로 있는 입력의 힌트
    - `text.disabled`: 비활성 컨트롤 글자
    - `accent.primary`, `accent.secondary`, `status.ok`: 글자에 쓰지 않는다. 바, 도트, 틱, 링, 선에만 쓴다.
    - `status.warn`, `status.error`, 범주 색 글자: `pane`, `elevated`, `hover`, `selected` 배경에만 쓴다. `chrome`과 `cross_highlight` 위에서는 도트나 틱과 기본 글자색을 쓴다.
26. 범주 색(Cat-1~5)은 글자나 6px 도트에만 쓰고 면을 채우지 않는다. Cat-5는 도트에만 쓴다.

## 모양

8. 배경이나 테두리가 있는 상자(배지, pill, 칩)는 누를 수 있는 컨트롤에만 쓴다. 메서드, 상태 코드, 콘텐츠 타입, 개수 같은 데이터 값은 상자 없는 글자로 쓴다.
9. 카드를 쓰지 않는다. 영역은 `borders.structural` 1px 선으로 나눈 도킹 패널이다. 테두리로 나뉜 패널 안에 테두리와 여백을 가진 상자를 또 넣지 않는다. 팝업, 메뉴, 대화 상자만 떠 있는 상자다.
10. 도트는 상태나 범주를 나타내는 인디케이터에만 쓰고 항상 글자 옆에 둔다. 장식용 도트, 글자 없는 도트를 쓰지 않는다.
11. 패널, 툴바, 표, 분할 영역, 상세 패널의 모서리는 0(`Radius::PANE`)이다. 버튼, 입력, 드롭다운, 팝업, 메뉴는 4pt(`Radius::CONTROL`)다. 단축키 칩과 스크롤바 썸만 2pt(`Radius::MICRO`)다. 양 끝이 반원인 pill 모양은 쓰지 않는다.
12. 이모지와 장식 아이콘을 쓰지 않는다. 대기 상태에서 깜빡이거나 숨 쉬거나 튀는 애니메이션을 쓰지 않는다. 계속되는 애니메이션은 진행 중인 작업 표시에만 쓴다.
13. 간격, 모서리, 글자 크기, 치수는 `tokens.rs`의 `Space`, `Radius`, `TypeScale`, `Layout` 값만 쓴다. 토큰 값이 다른 규칙을 어기면 화면 코드에서 우회하지 말고 `tokens.rs`의 값을 고친다. 새 값이 필요하면 `tokens.rs`에 더하고 PR 본문에 이유를 적는다.

### 아이콘

29. 아이콘 없이 글자 라벨만으로 모든 규칙을 지킬 수 있다. 아이콘을 들이면 14 또는 16px, 1.5px 라인 아이콘 세트 하나만 쓰고, 컨트롤의 글자 라벨을 대신할 때만 쓴다. 아이콘만 있는 버튼에는 1~3단어 툴팁과 접근성 라벨을 단다. 아이콘 글꼴이나 크레이트를 추가하는 PR은 라이선스를 적고 `cargo deny check`를 통과한다.

## 글

14. 컨트롤 라벨은 1~3단어의 명사나 동사다. 컨트롤 안에 문장을 넣지 않는다.
15. 화면에 상주하는 설명 문장은 0개다. 예외는 행동이 막혔거나 실패했을 때 그 자리에 보이는 한 줄뿐이다. 툴팁은 한 문장이고, 더 긴 설명은 `docs/guide/`에 둔다.
16. 제목, 부제, 도움말 문단, 단계별 설명처럼 글이 쌓이는 구조를 쓰지 않는다. 제목 위의 대문자 소제목을 쓰지 않고, 대문자는 `ID`, `URL` 같은 약어에만 쓴다.
17. 빈 상태는 문장 한 줄과 버튼 두 개 이하다.
18. 같은 숫자, 버튼, 라벨은 화면에 한 번만 보인다.
19. 면책·경고 문구도 상주시키지 않는다. 위험한 동작은 실행 직전의 확인 단계에서 한 줄로 알린다.
20. 화면 문구는 영어로 쓰고 **unslop** 스킬의 원칙을 따른다. 상투어, 인사말 같은 도입부, 챗봇식 문구를 쓰지 않는다.

### 서체

27. UI 글자는 sans, 데이터는 mono로 쓴다. 시간, 크기, 개수, 상태 코드, 포트처럼 바뀌거나 세로로 비교하는 숫자는 mono로 써서 자릿수 폭을 고정한다. 글자 크기와 굵기는 design-system.md의 타입 스케일을 따른다.
28. 서체를 바꾸거나 더하는 결정은 재디자인 PR 한 곳에서 내린다. 그 PR은 라이선스(OFL 여부), 바이너리 크기 변화, 현대 한글 11,172자 커버리지, 굵기 500·600 표현 방법을 비교해 본문에 적는다. 그 밖의 PR은 글꼴을 추가하지 않는다.

## 배치

21. 밀도를 우선한다. 표 행은 `Layout::TABLE_ROW_HEIGHT`(26pt), 표 헤더는 28pt, 트리 행은 24pt다. 행 전체를 클릭 영역으로 두어 24pt 이상의 대상 크기를 지킨다. 데이터 영역 안의 여백은 `Space::SM`(8pt) 이하다.
22. 온보딩 체크리스트, 번호 동그라미 단계, 히어로 영역, 아이콘·제목·설명으로 된 카드 묶음을 쓰지 않는다. 순서가 있는 설정은 컨트롤의 활성·비활성 상태와 막힌 이유 한 줄로 나타낸다.
23. 막대, 차트, 스파크라인은 값을 비교해야 할 때만 쓰고 숫자를 함께 보인다. 요약 카드처럼 이미 보이는 값을 다시 그리는 시각화를 만들지 않는다. 선은 1.5px 단색이고 면을 채우지 않는다.
24. 자주 쓰지 않는 기능은 메뉴에 넣는다. 모든 기능을 상단 바에 펼치지 않는다.

### 모션과 키보드

30. 호버, 선택, 포커스 전환은 즉시이거나 80ms 이하다. 패널 접기 같은 레이아웃 전환만 140ms 이하다. 테마는 애니메이션 없이 바꾼다.
31. 목록, 표, 트리는 ↑·↓, ←·→, Enter, Esc로 다룰 수 있다. 한 패널의 선택과 연관된 다른 패널의 항목은 `surfaces.cross_highlight` 배경으로 보인다.

### 워크벤치 레이아웃

32. 화면은 툴바(42pt), 선택적 보조 스트립, 주 패널과 인스펙터 분할, 상태 바(24pt) 순서다. 분할 패널마다 28pt 패널 헤더를 두고 왼쪽에 이름이나 탭, 오른쪽에 그 패널의 행동만 둔다. 스플리터는 1px 선과 5px 히트 영역, 스크롤바는 6pt다.
33. 표 헤더는 `surfaces.chrome` 배경으로 위에 고정한다. 행은 `borders.subtle` 1px 선으로 나누고 줄무늬를 쓰지 않는다. 근거는 design-system.md의 "데이터 표"에 있다. 선택 행은 `surfaces.selected` 배경과 왼쪽 2px `accent.primary` 바로 표시한다. 숫자, 크기, 시간 열은 오른쪽에 맞춘다.
34. 트리와 키-값 인스펙터는 깊이마다 16pt 들여쓰고 1px 가이드를 그린다. 키는 `text.secondary`, 값은 mono `text.primary`다. 원시 뷰어의 범위 강조는 테두리 없는 배경 하나다.
35. 버튼은 Ghost, Outline, Primary 세 종류이고 높이는 툴바 28pt, 패널 안 24pt다. 입력은 28pt이고, 포커스는 `accent.secondary` 테두리와 1px 바깥 선으로, 잘못된 값은 `status.error` 테두리와 오른쪽 끝 한 줄로 보인다.
36. 보조 스트립과 핵심 수치(`TypeScale::KPI`)는 표나 상태 바에 없는 값을 비교하거나 판단해야 할 때만 쓴다. 이미 보이는 개수를 크게 다시 적는 장식 숫자를 만들지 않는다. 수치마다 1~3단어 라벨을 붙인다.

## 이 저장소의 예

아래 나쁜 예는 `docs/screenshots/request-list-dark.png`(PR #228)와 PR #229의 `start-checklist.png`, `https-setup.png`에서 확인한 화면이다.

| 나쁜 예 | 어긴 규칙 | 좋은 예 |
| --- | --- | --- |
| 모든 행에 파란 `GET` 배지와 초록 `200 OK` pill이 있어 `403`·`500` 행이 묻힌다. 실패 행은 왼쪽 띠와 빨간 pill로 두 번 표시된다. | 1, 4, 8, 11 | `GET`과 `200 OK`는 `text.primary` 글자다. `403 Forbidden`은 `status.warn`, `500 Internal Server Error`는 `status.error` 글자색 하나로 표시한다. |
| 빈 화면의 `Start capturing` 카드에 부제, 번호 동그라미, `Done`·`Needed`·`Optional` pill, 단계별 설명 문단이 있다. | 9, 15, 16, 17, 22 | 표 자리에 `No requests yet. Proxy listening on 127.0.0.1:18930` 한 줄과 `Open Browser`, `Copy proxy URL` 버튼을 둔다. |
| HTTPS 설정 패널의 단계마다 2~3줄 설명이 있고 권한 경고가 늘 보인다. | 15, 19, 22 | `Create CA`, `Register CA`, `Restart with decryption`을 순서대로 두고 앞 단계가 끝나지 않은 항목은 비활성으로 둔다. 이유는 한 문장 툴팁으로 쓴다. 경고는 복호화를 켜는 확인 단계에서 한 번 보인다. |
| 요청 수가 `Requests 17`, `All 17`, `17 requests`로 세 번 보이고 `Open Browser`가 상단 바와 체크리스트에 두 번 있다. | 18 | 요청 수는 상태 바에만, `Open Browser`는 상단 바에만 둔다. |

## PR 전 점검 절차

작성 agent와 리뷰 agent가 같은 절차를 따른다.

1. 요소를 더하기 전에 같은 정보를 이미 보이는 곳이 있는지 찾는다.
2. 데이터가 있는 상태와 빈 상태를 테스트 트래픽으로 캡처한다. 색이나 스타일 토큰을 바꿨다면 라이트·다크 테마를 모두 찍는다.
3. 캡처에 보이는 설명 문장을 모두 센다. 막힘·실패 때 보이는 한 줄도 센다. 라벨과 값만 빼며 기준은 0개다. (15, 16, 19)
4. 아래 질문에 답한다. 모두 아니오여야 한다.
   - 정상 행이나 정상 상태에 의미 색이 있는가? (1, 2)
   - 데이터 값에 배경이나 테두리가 있는 상자가 있는가? (8)
   - 카드, 패널 안의 테두리 상자, 글자 없는 도트, 둥근 패널·표가 있는가? (9, 10, 11)
   - 같은 정보가 두 번 보이는가? (4, 18)
   - 바꾼 코드의 `on_hover_text`에 두 문장 이상인 툴팁이 있는가? (15)
   - 토큰 밖 색, 그림자, 그라데이션, 글로우, 줄무늬가 있는가? (5, 6, 33)
   - 대비 미달 토큰을 규칙 25의 용도 밖에 썼거나 `accent.*`·`status.ok`로 칠한 글자가 있는가? (7, 25)
   - 툴바·상태 바·패널 헤더·표·트리의 치수가 `Layout` 값과 다르거나 숫자 리터럴로 정해졌는가? 라이트와 다크 캡처에서 경계 위치가 다른가? (13, 21, 32)
   - 숫자, 크기, 시간이 sans이거나 왼쪽 정렬인가? (27, 33)
   - 장식 아이콘, 대기 중 애니메이션, 80ms를 넘는 상태 전환이 있는가? (12, 29, 30)
   - 30%를 지워도 잃는 정보가 없는 문구나 요소가 있는가? 있으면 지우고 다시 캡처한다.
   - DevTools Network 패널 옆에서 혼자 달라 보이는 요소가 있는가? 있으면 어긴 규칙을 찾는다.
5. PR 본문의 "UI 디자인 점검" 절에 설명 문장 수, 질문별 결과, 캡처를 적는다. 문장마다 규칙 15의 예외 근거(무엇이 막혔거나 실패했는지)를 적는다.
6. 리뷰 agent는 PR 캡처로 3·4단계를 다시 확인한다. 적힌 근거로 문장마다 규칙 15의 예외인지 확인한다. 위반은 규칙 번호를 인용해 코멘트하고 머지하지 않는다. UI를 바꾼 PR에 캡처가 없어도 머지하지 않는다.

## 출처

- 정밀 도킹 워크벤치 UI 디자인 시스템 명세서, Slate Navy & Muted Teal(사용자 제공, 값은 design-system.md에 옮김)
- [Anti-AI-slop ban list](https://github.com/Laith0003/ux-skill/wiki/Anti-AI-slop-ban-list), Laith0003 ux-skill
- [Spot the Slop: a UI designer's guide to fixing AI defaults](https://world.hey.com/kostac/spot-the-slop-a-ui-designer-s-guide-to-fixing-ai-defaults-4c448c9c), Kosta Canatselis
- [anti-ai-slop.md](https://raw.githubusercontent.com/referodesign/refero_skill/f78b4eccf112d7a179b92afeafdd7e8684560ac2/references/anti-ai-slop.md), Refero
- [ai-slop-patterns.md](https://cdn.jsdelivr.net/npm/pixelslop@0.3.8/dist/skill/resources/ai-slop-patterns.md), Pixelslop 0.3.8
- [Signs of a vibe-coded UI](https://www.thefountaininstitute.com/blog/signs-vibe-coded-ui), The Fountain Institute
- [WCAG 2.2](https://www.w3.org/TR/WCAG22/) 1.4.3 글자 대비, 1.4.11 글자 아닌 대비, 2.5.8 대상 크기
