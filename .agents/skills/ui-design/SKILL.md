---
name: ui-design
description: "HTTP Inspector의 UI 디자인 정책. 화면 설명 글을 없애는 원칙과 색, 모양, 글, 배치 규칙과 PR 전 캡처 점검 절차를 정한다. src/ui의 화면, 위젯, 스타일, 화면 문구를 추가하거나 바꿀 때와 UI를 바꾼 PR을 리뷰할 때 사용한다."
---

# UI 디자인 정책

HTTP Inspector는 개발 도구다. 기준은 DevTools Network 패널, Wireshark, mitmproxy, Charles다. 화면은 빽빽하고 평평하며 글자 중심이고, 색은 예외에만 쓴다.

가장 먼저 설명 글을 없앤다. 라벨, 값, 컨트롤 상태로 이해되는 화면에는 설명 문장을 쓰지 않는다. 이해되지 않으면 문장을 더하지 말고 라벨과 배치를 고친다. 색과 상자도 지웠을 때 잃는 정보를 말할 수 있을 때만 남긴다.

규칙 번호는 리뷰에서 인용하는 고정 식별자다. 삭제된 규칙의 번호는 빈 채로 둔다. 각 규칙은 캡처를 보고 예나 아니오로 판정한다.

현재 코드의 `Badge`(`badge.rs`), 왼쪽 띠(`ALERT_EDGE_WIDTH`), `Radius::CONTROL`(6pt), `Radius::CARD`, `ToneFills`는 규칙을 어기는 정리 대상이다. 코드에 있다는 사실은 다시 쓸 근거가 아니다.

## 색

1. 정상 상태는 기본 글자색으로 쓴다. 2xx 응답, `GET`·`POST` 같은 일반 메서드, 완료된 단계에 의미 색을 칠하지 않는다.
2. 의미 색은 예외와 상태 변화에만 쓴다. 4xx·5xx 응답, 연결 실패, 취소, 프록시 중지가 여기에 해당한다.
3. 강조색(`accent`)은 선택, 포커스, 화면의 주요 행동 하나에만 쓴다. 강조색으로 채운 버튼은 화면에 하나 이하다.
4. 같은 정보를 두 가지 색 표시로 반복하지 않는다. 실패 행이라면 왼쪽 띠와 상태 글자색 중 하나만 쓴다.
5. 그라데이션, 글로우, 그림자, 반투명 겹침, 의미 없는 장식 색을 쓰지 않는다.
6. 색 값은 `src/ui/style/tokens.rs`에만 정의한다. 화면 코드는 `Palette`나 `Tone`으로 의미를 고르고 `Color32::from_rgb`를 직접 쓰지 않는다.
7. 색만으로 의미를 전달하지 않는다. 상태는 `404 Not Found`처럼 글자로도 보인다. 의미 색 글자와 그 글자가 실제로 놓이는 배경의 대비는 라이트·다크 테마 모두 WCAG AA(4.5:1) 이상이다. `src/ui/style/theme.rs`의 `status_text_meets_aa_on_all_widget_backgrounds`가 이를 검사한다. 새 의미 색은 그 테스트의 글자색 목록에, 글자가 놓이는 새 배경은 `text_backgrounds()`에 추가한다.

## 모양

8. 배경이나 테두리가 있는 상자(배지, pill, 칩)는 누를 수 있는 컨트롤에만 쓴다. 메서드, 상태 코드, 콘텐츠 타입, 개수 같은 데이터 값은 상자 없는 글자로 쓴다.
9. 카드를 기본으로 쓰지 않는다. 영역은 여백과 1px 구분선(`Surfaces::border`)으로 나눈다. 테두리, 배경, 모서리를 지워도 내용을 이해할 수 있으면 카드가 아니다. 카드 안에 카드를 넣지 않는다.
10. 상태 점은 프록시 실행 상태처럼 실시간으로 바뀌는 상태 하나에만 쓴다. 화면에 상태 점은 하나 이하다.
11. 표 행, 목록, 상세 패널 같은 데이터 영역은 모서리를 둥글리지 않는다. 컨트롤과 팝업의 모서리는 4pt 이하이고, 양 끝이 반원인 pill 모양은 쓰지 않는다.
12. 이모지, 장식 아이콘, 애니메이션을 쓰지 않는다. 애니메이션은 진행 중인 작업 표시에만 쓴다.
13. 간격, 모서리, 글자 크기는 `tokens.rs`의 `Space`, `Radius`, `TypeScale` 값만 쓴다. 토큰 값이 다른 규칙을 어기면 화면 코드에서 우회하지 말고 `tokens.rs`의 값을 고친다. 새 값이 필요하면 `tokens.rs`에 더하고 PR 본문에 이유를 적는다. 글꼴은 추가하지 않는다.

## 글

14. 컨트롤 라벨은 1~3단어의 명사나 동사다. 컨트롤 안에 문장을 넣지 않는다.
15. 화면에 상주하는 설명 문장은 0개다. 예외는 행동이 막혔거나 실패했을 때 그 자리에 보이는 한 줄뿐이다. 툴팁은 한 문장이고, 더 긴 설명은 `docs/guide/`에 둔다.
16. 제목, 부제, 도움말 문단, 단계별 설명처럼 글이 쌓이는 구조를 쓰지 않는다.
17. 빈 상태는 문장 한 줄과 버튼 두 개 이하다.
18. 같은 숫자, 버튼, 라벨은 화면에 한 번만 보인다.
19. 면책·경고 문구도 상주시키지 않는다. 위험한 동작은 실행 직전의 확인 단계에서 한 줄로 알린다.
20. 화면 문구는 영어로 쓰고 **unslop** 스킬의 원칙을 따른다. 상투어, 인사말 같은 도입부, 챗봇식 문구를 쓰지 않는다.

## 배치

21. 밀도를 우선한다. 표 행 높이는 `src/ui/requests/request_table.rs`의 `ROW_HEIGHT`(24pt)를 넘지 않는다. 데이터 영역 안의 여백은 `Space::SM`(8pt) 이하다.
22. 온보딩 체크리스트, 번호 동그라미 단계, 히어로 영역, 아이콘·제목·설명으로 된 카드 묶음을 쓰지 않는다. 순서가 있는 설정은 컨트롤의 활성·비활성 상태와 막힌 이유 한 줄로 나타낸다.
23. 막대, 차트, 스파크라인은 값을 비교해야 할 때만 쓰고 숫자를 함께 보인다. 요약 카드처럼 이미 보이는 값을 다시 그리는 시각화를 만들지 않는다.
24. 자주 쓰지 않는 기능은 메뉴에 넣는다. 모든 기능을 상단 바에 펼치지 않는다.

## 이 저장소의 예

아래 나쁜 예는 `docs/screenshots/request-list-dark.png`(PR #228)와 PR #229의 `start-checklist.png`, `https-setup.png`에서 확인한 화면이다.

| 나쁜 예 | 어긴 규칙 | 좋은 예 |
| --- | --- | --- |
| 모든 행에 파란 `GET` 배지와 초록 `200 OK` pill이 있어 `403`·`500` 행이 묻힌다. 실패 행은 왼쪽 띠와 빨간 pill로 두 번 표시된다. | 1, 4, 8 | `GET`과 `200 OK`는 기본 글자색 글자다. `403 Forbidden`은 `warning`, `500 Internal Server Error`는 `error` 글자색 하나로 표시한다. |
| 빈 화면의 `Start capturing` 카드에 부제, 번호 동그라미, `Done`·`Needed`·`Optional` pill, 단계별 설명 문단이 있다. | 9, 15, 16, 17, 22 | 표 자리에 `No requests yet. Proxy listening on 127.0.0.1:18930` 한 줄과 `Open Browser`, `Copy proxy URL` 버튼을 둔다. |
| HTTPS 설정 패널의 단계마다 2~3줄 설명이 있고 권한 경고가 늘 보인다. | 15, 19, 22 | `Create CA`, `Register CA`, `Restart with decryption`을 순서대로 두고 앞 단계가 끝나지 않은 항목은 비활성으로 둔다. 이유는 한 문장 툴팁으로 쓴다. 경고는 복호화를 켜는 확인 단계에서 한 번 보인다. |
| 요청 수가 `Requests 17`, `All 17`, `17 requests`로 세 번 보이고 `Open Browser`가 상단 바와 체크리스트에 두 번 있다. | 18 | 요청 수는 상태 바에만, `Open Browser`는 상단 바에만 둔다. |

## PR 전 점검 절차

작성 agent와 리뷰 agent가 같은 절차를 따른다.

1. 요소를 더하기 전에 같은 정보를 이미 보이는 곳이 있는지 찾는다.
2. 변경한 화면을 실행해 데이터가 있는 상태와 빈 상태를 캡처한다. 색을 바꿨다면 라이트·다크 테마를 모두 찍는다. 테스트 트래픽만 쓴다.
3. 캡처에서 화면에 상주하는 설명 문장 수를 센다. 라벨, 값, 막힘·실패 때 보이는 한 줄, 툴팁은 세지 않는다. 기준은 0개다. (15, 16, 19)
4. 캡처를 보고 아래 질문에 답한다. 모두 아니오여야 한다.
   - 정상 행이나 정상 상태에 의미 색이 있는가? (1, 2)
   - 데이터 값에 배경이나 테두리가 있는 상자가 있는가? (8)
   - 카드, 둘 이상의 상태 점, 둥근 데이터 영역이 있는가? (9, 10, 11)
   - 같은 정보가 두 번 보이는가? (4, 18)
   - 지워도 잃는 정보가 없는 문구나 요소가 남아 있는가? 30%를 지워 보고, 잃는 정보가 없으면 지우고 다시 캡처한다.
   - DevTools Network 패널 옆에서 혼자 달라 보이는 요소가 있는가? 있으면 어긴 규칙을 찾는다.
5. PR 본문의 "UI 디자인 점검" 절에 설명 문장 수, 질문별 결과, 캡처를 적는다. 설명 문장이 0개가 아니면 문장마다 남긴 이유를 적는다.
6. 리뷰 agent는 PR 캡처로 3·4단계를 다시 확인한다. 설명 문장이 0개가 아니면 각 문장이 규칙 15의 예외인지 확인한다. 위반은 규칙 번호를 인용해 코멘트하고 머지하지 않는다. UI를 바꾼 PR에 캡처가 없어도 머지하지 않는다.

## 출처

- [Anti-AI-slop ban list](https://github.com/Laith0003/ux-skill/wiki/Anti-AI-slop-ban-list), Laith0003 ux-skill
- [Spot the Slop: a UI designer's guide to fixing AI defaults](https://world.hey.com/kostac/spot-the-slop-a-ui-designer-s-guide-to-fixing-ai-defaults-4c448c9c), Kosta Canatselis
- [anti-ai-slop.md](https://raw.githubusercontent.com/referodesign/refero_skill/f78b4eccf112d7a179b92afeafdd7e8684560ac2/references/anti-ai-slop.md), Refero
- [ai-slop-patterns.md](https://cdn.jsdelivr.net/npm/pixelslop@0.3.8/dist/skill/resources/ai-slop-patterns.md), Pixelslop 0.3.8
- [Signs of a vibe-coded UI](https://www.thefountaininstitute.com/blog/signs-vibe-coded-ui), The Fountain Institute
