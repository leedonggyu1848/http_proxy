# 정밀 도킹 워크벤치 디자인 시스템

HTTP Inspector UI의 색 토큰, 글자, 간격, 레이아웃 치수, 컴포넌트 규격을 정한다. 테마는 Slate Navy & Muted Teal이고 다크와 라이트 두 가지다. 규칙과 점검 절차는 [SKILL.md](SKILL.md)에 있고, 이 문서는 그 규칙이 가리키는 값을 담는다.

원문 명세는 CSS 변수로 쓰였다. egui 앱은 CSS를 읽지 않으므로 각 토큰을 `src/ui/style/tokens.rs`의 Rust 이름 하나에 대응시킨다. 표의 Rust 이름은 재디자인 PR이 만들 이름이다. 지금 코드에 있는 이름과 다르면 재디자인 PR에서 바꾼다.

## 원칙

- 화면은 1px 실선으로 나눈 평면 패널로 채운다. 패널을 둥근 카드로 띄우지 않는다.
- 면적의 95% 이상은 무채색과 딥 슬레이트다. 틸과 스카이 블루는 활성 상태, 선택 항목, 핵심 행동에만 쓴다.
- 다크와 라이트는 색만 다르다. 경계 위치, 위계, 치수는 1px도 다르지 않다.
- 그림자, 글로우, 배경 블러, 그라데이션, pill 배지, 대기 중 애니메이션, 패널 안의 테두리 상자를 쓰지 않는다. 팝업과 드롭다운 메뉴만 예외로 경계 그림자를 쓴다.

## 색 토큰

모든 색은 아래 토큰으로만 참조한다. 반투명 토큰은 `Color32::from_rgba_unmultiplied`로 만들고 알파는 `round(불투명도 × 255)`로 정한다.

### 표면과 경계

| 원문 토큰 | Dark | Light | Rust 이름 | 용도 |
| --- | --- | --- | --- | --- |
| `--bg-base` | `#0B1017` | `#F4F6F9` | `surfaces.base` | 최하위 캔버스, 비활성 영역 |
| `--bg-pane` | `#0F1620` | `#FFFFFF` | `surfaces.pane` | 요청 표, 상세, 원시 뷰어, 그래프 |
| `--bg-chrome` | `#141D29` | `#EEF2F6` | `surfaces.chrome` | 툴바, 패널 헤더, 표 헤더, 상태 바 |
| `--bg-elevated` | `#1A2534` | `#FFFFFF` | `surfaces.elevated` | 드롭다운, 툴팁, 팝업, 아웃라인 버튼 |
| `--bg-hover` | `rgba(148,163,184,0.08)` | `rgba(15,23,42,0.04)` | `surfaces.hover` | 행, 버튼, 트리 항목 호버 |
| `--bg-selected` | `rgba(45,212,191,0.10)` | `rgba(13,148,136,0.08)` | `surfaces.selected` | 선택 행, 켜진 토글 |
| `--bg-cross-highlight` | `rgba(56,189,248,0.18)` | `rgba(2,132,199,0.14)` | `surfaces.cross_highlight` | 교차 강조, 원시 뷰어 범위 강조 |
| `--border-structural` | `#1E2A3B` | `#D5DDE7` | `borders.structural` | 패널 경계, 툴바·상태 바 경계, 표 헤더 아래 |
| `--border-subtle` | `#16202E` | `#E6ECF2` | `borders.subtle` | 표 행 구분선, 트리 가이드, 줄 번호 경계 |
| `--border-control` | `#27364B` | `#C4CFDC` | `borders.control` | 입력, 셀렉트, 아웃라인 버튼 테두리, 스크롤바 썸 |

입력 필드 배경은 다크에서 `--bg-base`, 라이트에서 `--bg-elevated`다. 두 테마의 코드 경로를 같게 두려고 `surfaces.input`(Dark `#0B1017`, Light `#FFFFFF`) 하나로 둔다.

### 글자

| 원문 토큰 | Dark | Light | Rust 이름 | 용도 |
| --- | --- | --- | --- | --- |
| `--text-primary` | `#E2E8F0` | `#0F172A` | `text.primary` | 데이터 값, 활성 탭, 본문, 선택 행 글자 |
| `--text-secondary` | `#94A3B8` | `#475569` | `text.secondary` | 열 헤더, 라벨, 호스트, 상태 바 메타 |
| `--text-muted` | `#64748B` | `#8494A7` | `text.muted` | 규칙 25의 용도만 |
| `--text-disabled` | `#475569` | `#B0BCCB` | `text.disabled` | 비활성 컨트롤 글자 |

### 포인트와 의미 색

| 원문 토큰 | Dark | Light | Rust 이름 | 용도 |
| --- | --- | --- | --- | --- |
| `--accent-primary` | `#2DD4BF` | `#0D9488` | `accent.primary` | 선택 바, 켜진 토글 바, 활성 경로, 차트 선 |
| `--accent-secondary` | `#38BDF8` | `#0284C7` | `accent.secondary` | 포커스 링, 스플리터 호버·드래그 |
| `--status-ok` | `#34D399` | `#059669` | `status.ok`, `Tone::Success` | 연결됨, 실행 중 도트 |
| `--status-warn` | `#D9A05B` | `#B45309` | `status.warn`, `Tone::Warning` | 4xx, 취소, 지연 |
| `--status-error` | `#E06C75` | `#BE123C` | `status.error`, `Tone::Danger` | 5xx, 연결 실패, 프록시 중지 |

`Tone`은 `Success`, `Warning`, `Danger` 세 값만 남긴다. `Neutral`은 `text.secondary`로, `Info`는 쓰지 않는다. `ToneColors::fill`과 `ToneFills`는 없앤다.

### 주 실행 버튼

| 부분 | Dark | Light | Rust 이름 |
| --- | --- | --- | --- |
| 배경 | `#134E4A` | `#0F766E` | `primary_action.fill` |
| 글자 | `#5EEAD4` | `#FFFFFF` | `primary_action.text` |
| 테두리 | `#2DD4BF40` | 없음(`Color32::TRANSPARENT`) | `primary_action.stroke` |

### 범주 색

글자나 6px 도트에만 쓴다. 면을 채우지 않는다. Rust에서는 `Category` 열거형으로 고르고 `category(Category)`로 색을 얻는다.

| 원문 | Dark | Light | Rust 이름 |
| --- | --- | --- | --- |
| Cat-1 Blue-Slate | `#7DD3FC` | `#0369A1` | `Category::BlueSlate` |
| Cat-2 Muted Teal | `#5EEAD4` | `#0F766E` | `Category::Teal` |
| Cat-3 Muted Indigo | `#A5B4FC` | `#4338CA` | `Category::Indigo` |
| Cat-4 Warm Ochre | `#D4A373` | `#9A6324` | `Category::Ochre` |
| Cat-5 Cool Gray | `#94A3B8` | `#64748B` | `Category::Gray` |

## 대비

WCAG 2 대비비를 계산했다. 반투명 배경은 `--bg-pane` 위에 합성했다. 글자는 4.5:1, 도트·틱·바·포커스 링 같은 글자 아닌 표시는 3:1이 기준이다. `src/ui/style/tokens.rs`의 `contrast_ratio`와 같은 공식이다.

| 토큰 | Dark 최솟값 | Light 최솟값 | 글자 4.5:1 | 허용 용도 |
| --- | --- | --- | --- | --- |
| `text.primary` | 10.54 | 14.98 | 통과 | 제한 없음 |
| `text.secondary` | 5.07 | 6.36 | 통과 | 제한 없음 |
| `text.muted` | 2.73 (pane 3.82) | 2.60 (pane 3.10) | 미달 | 비활성 아이콘(글자 아님, 1.4.11 예외), 키-값 콜론, 줄 번호·오프셋 거터, 라벨이 따로 있는 입력의 힌트 |
| `text.disabled` | 1.71 | 1.62 | 미달 | 비활성 컨트롤 글자(WCAG 1.4.3 예외) |
| `accent.primary` | 6.98 | 3.14 | 라이트 미달 | 글자 금지, 표시만 |
| `accent.secondary` | 6.06 | 3.44 | 라이트 미달 | 글자 금지, 표시만 |
| `status.ok` | 6.76 | 3.16 | 라이트 미달 | 글자 금지, 도트만 |
| `status.warn` | 5.65 | 4.21 | chrome 4.46, 교차 강조 4.21 미달 | 데이터 표면의 글자 |
| `status.error` | 4.07 | 5.27 | 교차 강조 4.07 미달 | 데이터 표면의 글자 |
| Cat-1~4 | 5.75 | 4.21 | Cat-4 chrome 4.46, 교차 강조 4.21 미달 | 데이터 표면의 글자, 도트 |
| Cat-5 | 5.07 | 3.99 | 라이트 hover 4.40, selected 4.32 미달 | 도트만 |

데이터 표면은 `pane`, `elevated`, `hover`, `selected`다. 이 네 배경에서는 `status.warn`, `status.error`, Cat-1~4가 모두 4.5:1을 넘는다. `chrome`과 `cross_highlight` 위에서는 의미를 도트나 레프트 틱으로 보이고 글자는 `text.primary`나 `text.secondary`로 쓴다.

`accent.*`와 `status.ok`는 모든 표면에서 3:1을 넘으므로 표시에는 쓸 수 있다. 다크의 글자 대비는 충분하지만 두 테마의 구조를 같게 두려고 두 테마 모두 글자에 쓰지 않는다. `borders.*`는 표면 대비 1.0~1.6이다. 경계선은 영역 구분용이고, 컨트롤은 글자 라벨로 알아본다.

주 실행 버튼 글자는 다크 6.41, 라이트 5.47로 통과한다.

## 글자

UI 글자는 sans, 데이터는 mono로 나눈다. egui의 `FontFamily::Proportional`이 sans, `FontFamily::Monospace`가 mono다. 시간, 크기, 개수, 상태 코드, 포트처럼 바뀌거나 세로로 비교하는 숫자는 mono로 써서 자릿수 폭을 고정한다(CSS의 `tabular-nums`에 해당).

| 역할 | 크기/행간 | 굵기 | 서체 | Rust 이름 |
| --- | --- | --- | --- | --- |
| 패널 제목, 툴바 제목 | 12/16 | 600 | sans, 자간 0.01em | `TypeScale::PANEL_TITLE`, `TypeScale::PANEL_TITLE_TRACKING` |
| 컨트롤, 버튼, 탭 | 12/16 | 500 | sans, 줄바꿈 없음 | `TypeScale::CONTROL` |
| 표 열 헤더 | 11/16 | 600 | sans, `text.secondary` | `TypeScale::COLUMN_HEADER` |
| 본문 | 12.5/18 | 400 | sans | `TypeScale::BODY` |
| 데이터 셀, 수치, 코드 | 12/18 | 400 | mono | `TypeScale::DATA` |
| 핵심 수치 | 16/20 | 600 | mono | `TypeScale::KPI` |
| 상태 바, 보조 메타 | 11/14 | 400 | mono, `text.secondary` | `TypeScale::STATUS` |

원문 서체는 sans `Pretendard`, mono `JetBrains Mono`다. 현재 앱은 egui 기본 글꼴에 Noto Sans KR 서브셋(`assets/fonts/`)을 이어 붙인다. egui는 굵기 축이 없어서 500·600은 별도 글꼴 파일을 등록해야 한다. 서체와 굵기는 규칙 28대로 재디자인 PR에서 정한다. 행간은 여러 줄 글자(본문, 원시 뷰어)에서 `TextFormat::line_height`로 적용한다. 패널 제목의 자간 0.01em은 12pt × 0.01 = 0.12pt이고, `TextFormat::extra_letter_spacing`에 `TypeScale::PANEL_TITLE_TRACKING`(0.12)을 넣는다.

## 간격과 곡률

| 원문 | 값 | Rust 이름 |
| --- | --- | --- |
| 간격 | 2 | `Space::XXS` |
| 간격 | 4 | `Space::XS` |
| 간격 | 6 | `Space::S` |
| 간격 | 8 | `Space::SM` |
| 간격 | 12 | `Space::MD` |
| 간격 | 16 | `Space::LG` |
| `--radius-pane` | 0 | `Radius::PANE` |
| `--radius-control` | 4 | `Radius::CONTROL` |
| `--radius-micro` | 2 | `Radius::MICRO` |

- `Radius::PANE`: 패널, 툴바, 표, 분할 영역, 상세 패널
- `Radius::CONTROL`: 버튼, 입력, 드롭다운, 세그먼트 컨트롤, 팝업, 메뉴, 대화 상자
- `Radius::MICRO`: 단축키 칩, 스크롤바 썸
- 상태 도트는 원(`circle_filled`)으로 그린다.

## 레이아웃 치수

원문이 범위로 준 값은 원문 템플릿 값이나 범위 안의 한 값으로 고정했다. 모든 값은 `tokens.rs`의 `Layout`에 둔다.

| 요소 | 원문 | 값 | Rust 이름 |
| --- | --- | --- | --- |
| 툴바 높이 | 40~44 | 42 | `Layout::TOOLBAR_HEIGHT` |
| 보조 스트립 높이 | 110~140 | 120 | `Layout::AUX_STRIP_HEIGHT` |
| 주 패널 기본 폭 | 60~65% | 0.6 | `Layout::PRIMARY_PANE_FRACTION` |
| 상태 바 높이 | 24~26 | 24 | `Layout::STATUS_BAR_HEIGHT` |
| 패널 헤더 높이 | 28~30 | 28 | `Layout::PANE_HEADER_HEIGHT` |
| 스플리터 히트 영역 | 5 | 5 | `Layout::SPLITTER_HIT` |
| 스크롤바 폭 | 6 | 6 | `Layout::SCROLLBAR_WIDTH` |
| 표 헤더 높이 | 28 | 28 | `Layout::TABLE_HEADER_HEIGHT` |
| 표 행 높이 | 26~28 | 26 | `Layout::TABLE_ROW_HEIGHT` |
| 트리 행 높이 | 24 | 24 | `Layout::TREE_ROW_HEIGHT` |
| 트리 들여쓰기 | 16 | 16 | `Layout::TREE_INDENT` |
| 툴바 버튼 높이 | 28 | 28 | `Layout::TOOLBAR_BUTTON_HEIGHT` |
| 패널 안 버튼 높이 | 24 | 24 | `Layout::PANE_BUTTON_HEIGHT` |
| 입력 높이 | 28 | 28 | `Layout::INPUT_HEIGHT` |
| 버튼 좌우 패딩 | 8~10 | 8 | `Space::SM` |
| 상태 도트 지름 | 6 | 6 | `Layout::DOT_SIZE` |
| 레프트 틱 | 2×12 | 2×12 | `Layout::TICK_WIDTH`, `Layout::TICK_HEIGHT` |
| 선택 바 폭 | 2 | 2 | `Layout::SELECTION_BAR_WIDTH` |
| 쉐브론 | 12 | 12 | `Layout::CHEVRON_SIZE` |
| 아이콘 | 14, 16 | 14, 16 | `Layout::ICON_SMALL`, `Layout::ICON` |

화면은 위에서부터 다음 슬롯으로 나눈다. 슬롯 사이는 `borders.structural` 1px 선이다.

1. 툴바: 높이 42, 배경 `surfaces.chrome`
2. 보조 스트립: 선택 사항이고 접을 수 있다. 규칙 36의 조건을 만족할 때만 둔다.
3. 주 패널과 인스펙터: 기본 폭 6:4로 나누고 스플리터로 조절한다. 인스펙터는 위아래로 다시 나눌 수 있다.
4. 상태 바: 높이 24, 배경 `surfaces.chrome`

각 분할 패널의 맨 위에는 패널 헤더를 둔다. 배경은 `surfaces.chrome`, 아래 경계는 `borders.structural` 1px이다. 왼쪽에는 패널 이름이나 탭, 오른쪽에는 그 패널의 작은 행동 버튼만 둔다.

스플리터는 1px `borders.structural`로 보이고 5px 영역에서 포인터를 받는다. 호버나 드래그 중에만 `accent.secondary`로 바뀐다. 스크롤바는 폭 6, 트랙 투명, 썸 `borders.control`이다.

## 컴포넌트

### 버튼

| 종류 | 배경 | 테두리 | 글자 | 용도 |
| --- | --- | --- | --- | --- |
| Ghost | 투명, 호버 `surfaces.hover` | 없음 | `text.primary` | 기본 |
| Ghost 켜짐 | `surfaces.selected` | 아래 2px `accent.primary` 바 | `text.primary` | 토글, 활성 탭 |
| Outline | `surfaces.elevated` | 1px `borders.control` | `text.primary` | 보조 행동 |
| Primary | `primary_action.fill` | `primary_action.stroke` | `primary_action.text` | 화면의 가장 중요한 행동 하나 |

원문은 켜진 토글 글자를 `--accent-primary`로 칠한다. 라이트 `surfaces.selected` 위에서 3.41:1이므로 글자는 `text.primary`로 두고 켜짐은 아래 2px 바로 표시한다. 높이는 툴바 28, 패널 안 24다. 그림자를 넣지 않는다.

### 입력과 필터 줄

높이 28, 배경 `surfaces.input`, 테두리 1px `borders.control`, 모서리 `Radius::CONTROL`이다. 포커스되면 테두리를 `accent.secondary`로 바꾸고 바깥에 1px 선을 더한다. 흐린 글로우는 쓰지 않는다. 값이 잘못되면 테두리를 `status.error`로 바꾸고 입력 오른쪽 끝에 11pt 오류 글자를 한 줄 둔다. 이 한 줄은 규칙 15의 실패 예외로 센다.

### 상태·범주 인디케이터

둘 중 하나만 쓴다. 상자와 배경은 없다.

- 도트와 글자: 6px 원 + `Space::S` + `text.primary`나 `text.secondary` 글자. 기본형이다.
- 레프트 틱: 2×12 세로 바 + `Space::S` + mono 글자. 표 행처럼 강조가 필요한 곳에 쓴다.

### 데이터 표

- 헤더: 높이 28, 배경 `surfaces.chrome`, 아래 1px `borders.structural`, 스크롤해도 위에 고정한다.
- 행: 높이 26, 아래 1px `borders.subtle`. 줄무늬(`striped`)를 쓰지 않는다. 원문은 강한 줄무늬만 금지하지만 두 가지 이유로 약한 줄무늬도 뺐다. 행마다 있는 `borders.subtle` 선이 이미 행을 나누므로 줄무늬는 같은 정보의 두 번째 표시다(규칙 4). 원문에 줄무늬 색 토큰이 없어 어떤 줄무늬 색도 토큰 밖 색이 된다(규칙 6).
- 선택: 배경 `surfaces.selected`, 행 왼쪽 끝에 2px `accent.primary` 세로 바, 글자는 `text.primary`. 배경과 바는 한 표시다. 실패 행의 레프트 틱은 행 끝이 아니라 상태 셀의 글자 앞에 두어 선택 바와 겹치지 않는다.
- 정렬: 글자는 왼쪽, 숫자·크기·시간은 오른쪽에 맞추고 mono로 쓴다.

### 트리와 키-값 인스펙터

깊이마다 16 들여쓰고 깊이마다 왼쪽에 1px `borders.subtle` 가이드를 그린다. 행 높이는 24, 왼쪽에 12px 쉐브론(▸, ▾)을 둔다. 키는 `text.secondary`, 콜론은 `text.muted`, 값은 mono `text.primary`다.

### 원시 뷰어

배경은 `surfaces.pane`이다. 줄 번호나 오프셋 열은 `text.muted`이고 오른쪽에 1px `borders.subtle` 경계를 둔다. 범위 강조는 `surfaces.cross_highlight` 배경이고 테두리를 두지 않는다.

### 시각화

- 선 차트: 격자는 최소한의 1px 점선 `borders.subtle`, 데이터 선은 1.5px `accent.primary` 단색이다. 선 아래 면을 채우지 않는다.
- 관계도: 비활성 노드와 선은 `borders.control`과 `text.muted`로 누른다. 선택·활성 경로만 1.5px `accent.primary` 실선이고 대기·보조 경로는 3-3 점선이다.

### 아이콘

아이콘은 선택 사항이다. 넣는다면 14 또는 16px, 선 굵기 1.5px의 라인 아이콘 세트 하나만 쓴다. 도입 조건은 규칙 29에 있다.

## 인터랙션과 모션

- 호버, 선택, 포커스 전환은 즉시 바꾸거나 80ms 이하 ease-out이다. egui `Style::animation_time` 기본값 0.2초를 0.08초 이하로 낮춘다.
- 패널 접기·펼치기처럼 레이아웃이 바뀌는 전환만 140ms 이하다.
- 테마는 애니메이션 없이 즉시 바꾼다.
- 목록, 표, 트리는 ↑·↓로 이동하고 ←·→로 접고 펼친다. Enter로 고르고 Esc로 선택을 풀거나 검색창을 빠져나간다.
- 한 패널에서 고른 항목과 연관된 다른 패널의 항목에 `surfaces.cross_highlight` 배경을 깐다.

## egui 설정 대응

| 설정 | 값 |
| --- | --- |
| `Visuals::window_shadow` | `Shadow::NONE` |
| `Visuals::popup_shadow` | offset 0, spread 0, blur 4 이하 |
| `Visuals::window_corner_radius`, `menu_corner_radius` | `Radius::CONTROL` |
| `Visuals::striped`, `Grid::striped`, `TableBuilder::striped` | `false` |
| `Visuals::selection.bg_fill` | `surfaces.selected` |
| `Visuals::selection.stroke` | `accent.primary`. 글자색으로 쓰지 않는다. |
| `Visuals::hyperlink_color` | `text.primary`. 링크는 밑줄로 구분한다. |
| `Spacing::scroll` | `ScrollStyle::solid()` 기반, `bar_width` 6 |
| `Style::animation_time` | 0.08 이하 |
