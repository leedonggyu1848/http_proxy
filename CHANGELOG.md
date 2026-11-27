# 변경 기록

이 파일은 사용자가 체감하는 변경을 버전별로 기록한다. 형식은 [Keep a Changelog 1.1.0](https://keepachangelog.com/ko/1.1.0/)을, 버전 번호는 [유의적 버전(SemVer) 2.0.0](https://semver.org/lang/ko/)을 따른다. 작성 규칙과 릴리스 절차는 [개발 문서의 릴리스와 버전](docs/development.md#릴리스와-버전)에 있다.

## [Unreleased]

### 추가

- Linux 상세 패널의 **Process** 탭에서 요청을 보낸 프로그램, 부모 프로세스, 연결 정보를 확인하고 같은 프로세스의 요청만 볼 수 있다. 명령줄의 민감 인자는 화면에서 가리고 전체 로그에는 프로그램 이름, PID, 실행 파일 경로를 기록한다.
- 프록시와 CA 신뢰 설정이 자동 주입된 검사 전용 셸을 추가한다. 툴바의 **Open Terminal**(**터미널 열기**)은 설치된 터미널(`$TERMINAL`, `x-terminal-emulator`, GNOME Terminal, Konsole, Xfce Terminal, kitty, Alacritty, WezTerm, foot, xterm, macOS Terminal.app)에서 셸을 열고, 명령줄의 `http-proxy shell`은 대화형 서브셸을, `http-proxy exec -- <명령>`은 명령 하나를 같은 환경으로 실행해 종료 코드를 그대로 돌려준다. 셸 안의 `curl`, Python, Node.js, `git` 등은 옵션 없이 캡처된다. `HTTP_PROXY`·`HTTPS_PROXY`·`ALL_PROXY`(소문자 포함)를 프록시 주소로 정하고 `NO_PROXY`를 지워 로컬 테스트 서버도 캡처하며(`--keep-no-proxy`로 유지), HTTPS 복호화가 켜져 있으면 시스템 루트 CA 뒤에 로컬 공개 인증서를 덧붙인 임시 묶음을 `SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `REQUESTS_CA_BUNDLE`, `GIT_SSL_CAINFO`, `AWS_CA_BUNDLE`, `DENO_CERT`에 지정해 프록시를 거치지 않는 HTTPS도 깨지지 않는다. 묶음은 사용자 전용 `0700` 디렉터리의 `0600` 파일이며 셸이 끝나면 지운다. 시스템 CA 저장소, 방화벽, 셸 설정 파일은 바꾸지 않는다. 검사 셸과 `proxy env --ca-bundle`은 이미 있는 공개 인증서 `root-ca.pem`만 읽고 CA 개인키 파일을 열거나 CA를 새로 만들지 않으며, CA가 없으면 `https_unavailable`로 실패한다. [검사 터미널](docs/guide/proxy-and-capture.md#검사-터미널)
- 요청 목록에 각 요청의 상대 시작 시각과 연결·전송·대기·수신 단계별 기간을 막대로 보여 주는 `Waterfall`(`워터폴`) 열을 추가한다. 관측하지 못한 구간은 0이 아닌 빈 구간으로 표시하며, 상세 패널의 **Timing** 탭과 같은 타이밍 데이터를 사용한다. 막대에 마우스를 올리면 시작 시각과 단계별 수치를 표로 확인할 수 있고, 열 헤더를 클릭해 시작 시각 기준으로 정렬할 수 있다.
- URL 맵의 관계 그래프(**Graph**) 오른쪽에 접이식 도킹 범례 패널(**Legend**)을 추가한다. 노드, 요청 수 로그 비율 크기, 1st/3rd-party 테두리, Referer·Origin (CORS)·Redirect (3xx) 선과 화살표 방향, 강조 경로, 고리 선 견본과 드래그·클릭 조작 방법을 확인할 수 있다. 1px 스플리터로 폭을 조절하고 세션 동안 열림 여부와 폭을 유지하며, 토글·폭 조절 시 그래프 배치와 선택 상태가 유지된다. **Edges ▼** 메뉴는 선 종류 표시 토글만 남기고 견본은 범례 패널로 옮긴다.
- 명령줄 인터페이스를 추가한다. `http-proxy proxy start|stop|status|set|env`, `records list|show|body|watch|wait|clear|replay|curl|export|endpoints|hls`, `ca info`, `log save`, `schema`로 X11 디스플레이 없이 프록시를 띄우고 캡처한 요청·응답을 읽고, 고쳐서 재전송하고, `curl` 명령으로 옮긴다. `records export`는 앱과 같은 HAR 1.2를, `records endpoints`는 Map 상세와 같은 엔드포인트별 요청·응답 구조와 OpenAPI 3.1 문서를 만들고, `records hls`는 캡처한 HLS 재생목록의 화질을 확인해 `.ts` 파일로 받는다(진행은 NDJSON, 기존 파일은 덮어쓰지 않음). AI 에이전트용 안내는 `.agents/skills/http-inspector-cli/SKILL.md`에 있다. `records replay`는 앱과 같은 확인 규칙을 따르며 서버 상태를 바꿀 수 있는 요청은 `--yes`가 없으면 보내지 않는다(종료 코드 5). `--dry-run`은 보낼 요청만 출력한다. `proxy env`는 셸에 적용할 프록시·CA 환경 변수를 출력하고, `ca info`는 CA 인증서 경로·지문·유효 기간만 출력한다. `records list`는 기본으로 요약 필드만 출력하고 `--full`로 모든 필드를 출력한다. 검사 브라우저, CA 등록, URL 맵처럼 화면에 맞는 기능은 앱에서만 제공한다. 모든 명령은 `--json`으로 `"schema": 1`이 있는 JSON(감시는 NDJSON)을 출력하고 문서화한 종료 코드를 반환한다. 필터 문법과 본문 Pretty·Raw·Tree 결과는 앱과 같다. 백그라운드 데몬은 현재 사용자만 열 수 있는 Unix 소켓으로 제어한다. 인자 없이 실행하면 지금처럼 앱이 열린다. [명령줄 인터페이스](docs/cli.md)
- 한국어 UI를 지원한다. 툴바의 **Settings**(**설정**) 메뉴에서 **System**(**시스템**), **English**, **한국어**를 선택해 재시작 없이 즉시 전환할 수 있으며, 기본값은 시스템 로케일(`LC_ALL` → `LC_MESSAGES` → `LANG`)을 따른다.
- 캡처한 HLS 재생목록(`.m3u8`)에서 화질별 미디어 재생목록과 세그먼트 목록을 확인하고, 현재 세션에 선택한 화질의 Chrome `.ts` 세그먼트 요청이 있을 때 빠진 세그먼트를 받아 재생목록 순서대로 하나의 `.ts` 파일로 저장한다. 이미 보관한 세그먼트는 재사용하며, 진행률 표시·취소·실패 상태 안내를 제공한다.
- 요청 필터에 `method:POST url:/api/users status:4xx`처럼 여러 조건을 조합하는 문법을 추가한다. 메서드·호스트·상태 자동 완성과 우클릭 필터를 지원한다.
- Linux x86_64용 `.deb` 패키지(`http-inspector`)를 배포한다. 실행 파일을 `/usr/bin/http-proxy`에 설치하고 애플리케이션 메뉴 항목과 아이콘을 등록한다.
- 첫 공개 버전에 포함될 기능: HTTP/1.1 요청·응답 스트리밍 기록, 선택적 HTTPS 복호화와 검사 브라우저, 요청 재전송과 cURL 복사, URL 맵과 관계 그래프, 로그·본문·HAR 1.2 저장.

### 변경

- 평문 HTTP와 복호화 HTTPS 전달 경로가 업스트림 HTTP/1.1 연결 풀을 함께 사용해 같은 호스트·TLS 검증 설정으로 이어지는 요청의 TCP·TLS 연결을 재사용한다. 유휴 연결은 호스트별 수량과 타임아웃으로 제한하고 프록시를 중지하면 모두 닫으며, 유휴 연결이 업스트림에서 닫힌 경우에는 멱등 요청만 새 연결로 한 번 재시도하고 비멱등 요청(`POST` 등)은 중복 전송하지 않는다. 기록에는 연결 ID와 재사용 여부가 남고, 재사용한 요청의 Timing `connect` 구간은 `None`(`—`)으로 표시한다. HAR 1.2 내보내기는 `connection` 필드를, CLI `records show`는 `connection` 정보를 포함한다.
- `http-proxy proxy env --ca-bundle`이 시스템 CA 묶음을 로컬 CA 하나로 바꾸지 않고, 시스템 루트와 로컬 CA를 합친 묶음(제어 디렉터리의 `combined-ca.pem`, `proxy stop` 때 삭제)을 `SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `REQUESTS_CA_BUNDLE`, `GIT_SSL_CAINFO`, `AWS_CA_BUNDLE`, `DENO_CERT`에 지정한다. `proxy env`는 `ALL_PROXY`, `all_proxy`, `HTTP_INSPECTOR_PROXY`도 내보낸다.
- 요청 목록의 열 이름이 `Host / Path`에서 `Path`로 바뀐다. 경로를 먼저 보여 주고 호스트는 흐린 글자로 붙이며, 모든 요청이 같은 호스트면 호스트를 열 제목 옆에 한 번만 표시한다. 긴 경로는 가운데를 줄인다. 상세 패널의 헤더·쿼리 이름은 흐린 글자로 표시해 값이 먼저 보이게 한다.
- 화면을 정밀 도킹 워크벤치 디자인(Slate Navy & Muted Teal)으로 바꾼다. 밝은 테마와 어두운 테마의 색, 모서리, 글자 크기를 새로 정하고 그림자와 줄무늬를 없앤다.
- 요청 목록에서 메서드와 상태를 상자 없는 글자로 표시한다. 정상 응답은 기본 글자색이고, 4xx·5xx·실패 요청만 상태 글자 앞의 짧은 세로 표시로 구분한다. 행은 26pt 높이에 가는 선으로 나뉘고, 선택한 행은 옅은 청록 배경과 왼쪽 막대로 표시한다. 크기와 시간은 오른쪽에 맞춘다.
- 툴바, 알림, 상태 바의 상태와 경고는 글자 앞의 점 색으로 구분하고 글자는 기본 글자색으로 쓴다. 상세 패널의 상태 글자는 예외일 때만 색을 쓴다.
- 빈 목록과 선택하지 않은 상세에 보이던 안내 문장을 한 줄 상태(`No requests yet · proxy http://…`, `No request selected`)로 줄인다.
- 화면 틀을 툴바(42pt)·패널 머리줄(28pt)·상태 바(24pt)로 나눈다. 툴바 왼쪽에 앱 이름, 프록시 주소, HTTPS 상태를, 가운데에 **Requests**·**Map** 전환을, 오른쪽에 왼쪽부터 요청 필터 입력란, **Save**, **Settings**, **Open Terminal**을, 맨 오른쪽에 주 행동 버튼 하나(중지 중 **Start**, 실행 중 **Open Browser**)를 둔다. 창이 1080pt보다 좁으면 보기 전환과 필터를 툴바 둘째 줄로 옮긴다.
- 프록시의 복사 항목과 **Stop**은 툴바의 프록시 주소 메뉴로, **Decrypt HTTPS**와 CA 상태·**Register CA**·**CA details**는 `HTTPS on/off` 메뉴로, 캡처 범위 안내(**?**)와 cURL 복사의 **Redacted placeholders**는 **Settings** 메뉴로 옮긴다. 신뢰된 CA의 **Remove CA**는 **CA details** 하위 메뉴에만 둔다.
- 요청 결과 종류(All·Responses·Failures·Tunnels)를 드롭다운 대신 목록 머리줄의 글자 탭으로 고르고, **Follow newest**·**Clear**를 같은 줄 오른쪽에 둔다. 상세의 **Headers**·**Body** 탭과 **Edit and Resend**·**Copy as cURL**도 상세 머리줄에 둔다.
- 요청 수·호스트·노드 수를 보기 전환 대신 상태 바에 한 번만 표시하고, 실패한 요청 수와 검색 진행도 상태 바에 표시한다. 상태 바 경고는 짧은 이름으로 보이고 마우스를 올리면 설명을 보여 준다.
- 잘못된 정규식은 필터 입력란의 빨간 테두리와 오른쪽 끝 `Invalid pattern`으로 표시한다. 입력란에 포커스가 있으면 파란 테두리로 보인다. 스크롤바는 바탕 없는 6pt 손잡이로 바뀐다.
- 목록과 상세의 기본 너비 비율을 62:38로 바꾸고, 가운데 경계를 1px 선과 5pt 끌기 영역으로 줄인다.
- 포트 충돌·권한 오류 안내와 CA 등록 후 안내를 한 문장으로 줄이고, CA 등록 후 안내에 **Dismiss**를 둔다.
- 상세 패널의 **Headers** 탭을 요청 줄과 **Query**·**Request**·**Response**(터널은 **Tunnel**) 묶음의 키-값 트리로 바꾼다. 묶음은 접고 펼칠 수 있고, 키는 한 열에 맞추며 마스킹한 값은 흐린 글자로 보인다. 연결 단계별 시간은 모든 탭 위에서 새 **Timing** 탭으로 옮긴다. 결과 줄은 서버가 보낸 reason phrase와 HTTP 버전을 함께 보여 준다(`502 Bad Gateway · HTTP/1.1`).
- **Body** 탭에서 요청·응답 본문마다 이름, 형식, 크기와 **Save ▼** 메뉴가 있는 머리줄을 두고, 본문에 줄 번호 열을 붙인다. 보기 방식은 **Pretty**·**Raw**·**Tree** 토글로 고른다. `119 bytes observed · complete · full preview`와 `Format: JSON · Pretty` 줄은 머리줄의 형식·크기로 대신하고, 잘린 미리보기 안내는 짧은 한 줄로 줄인다.
- 구문 강조 색을 디자인 토큰의 범주 색(키·태그 Cat-1, 문자열 Cat-3, 숫자·리터럴 Cat-4)과 보조 글자색으로 바꾼다. 검색 일치는 노란 배경 대신 교차 강조 배경으로, 현재 일치는 밑줄을 더해 표시하고 이동 버튼은 `1/3` **Previous**·**Next**로 줄인다.
- URL 맵의 **Tree**·**Graph** 전환과 **Export**·**Clear**를 맵 패널 머리줄로, 필터는 머리줄 아래 한 줄로 옮긴다. 트리 행은 24pt 높이에 깊이마다 가이드선을 그리고, 상태 비율 막대 대신 오류 수와 요청 수를 열을 맞춘 숫자로 보여 준다. 상세 패널은 노드 URL과 **Show in Requests**가 있는 머리줄 아래 **Summary**·**Endpoints**·**Query names**·**Requests** 묶음으로 바뀐다.
- 관계 그래프는 평소 노드와 선을 흐리게 두고 마우스를 올리거나 Tree에서 고른 도메인과 그 선만 청록 실선으로 강조한다. Origin은 3-3 점선, Redirect는 짧은 점선이다. 그래프 위 범례 줄은 머리줄의 **Edges ▼** 메뉴(선 종류 켜기·끄기)로 옮기고, 도메인·선 수는 상태 바에 표시한다.
- 창이 좁아 상태 바에 다 들어가지 않으면 덜 중요한 항목부터 `+N`으로 줄이고, `+N`에 마우스를 올리면 숨은 항목을 보여 준다.
- 맵 호스트 필터, JSON 트리 검색, HAR 경로, **Edit and Resend**의 URL·헤더·본문 입력란을 툴바 필터와 같은 입력란으로 바꿔 포커스 테두리를 통일한다. 체크박스·라디오와 스크롤바 손잡이는 마우스를 올리면 밝아진다.
- **Capture scope** 메뉴의 설명 문장을 기록하는 것·기록하지 않는 것의 짧은 목록으로 줄이고, 터널 행과 상세의 긴 안내는 한 문장 툴팁으로 바꾼다. 맵 엔드포인트·그래프·본문 트리의 상주 설명 문장을 없앤다.

### 수정

- 다크 모드에서 Map 관계 그래프의 기본 간선과 화살표가 배경에 묻혀 읽히지 않던 문제를 수정한다. 비활성 간선(1.25px)과 노드 테두리(1px), **Edges ▼** 메뉴의 범례 견본에 같은 흐린 회색을 써서 마우스를 올리지 않아도 연결 방향과 선 패턴(`Referer`, `Origin`, `Redirect`)을 구분할 수 있게 하고, 호버 시 청록색 강조 경로가 뚜렷하게 드러나도록 한다.
- 전체 로그의 임시 파일을 만들자마자 이름을 지워, 앱이나 명령줄 데몬이 비정상 종료되거나 강제로 끝나도 본문이 담긴 임시 파일이 임시 디렉터리에 남지 않게 한다.

- 프록시를 중지할 때 열려 있던 `CONNECT` 터널이 `Open` 상태로 남거나 전체 세션 로그에 종료 기록이 누락되던 문제와, 종료 중 진행 중인 요청·터널이 간헐적으로 실패로 기록되던 문제를 수정한다.
