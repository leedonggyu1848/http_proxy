# HTTP Inspector

HTTP Inspector는 로컬 HTTP 프록시를 통해 요청 기록을 보여주는 Linux와 macOS 데스크톱 앱이다. UI와 프록시 엔진은 모두 Rust로 작성했다.

## MVP 기능

현재 버전은 다음 기능을 제공한다.

- 루프백 주소의 HTTP 프록시(기본 포트 `8080`, 변경 가능)
- HTTP/1.1 요청과 응답 스트리밍
- HTTPS `CONNECT` TCP 터널
- 사용자가 켤 수 있는 HTTPS 복호화
- 설치별 로컬 CA 생성과 제한된 개인키 저장
- 버튼으로 현재 사용자의 Chrome·Chromium CA 등록, 상태 확인, 제거
- 설치된 Firefox·Chrome·Chromium 선택과 프록시가 설정된 비공개 검사 창 실행
- 실제 HTTPS 서버 인증서와 호스트 이름 검증
- 메서드, 대상, 결과, 응답 헤더 도착 시간 표시
- 요청과 응답 헤더 상세 보기
- 방향별 최대 8KiB의 실시간 본문 미리보기
- `Content-Type: image/png` 응답 본문의 이미지 미리보기와 16진수 보기
- 문자열 검색과 응답, 실패, 터널 결과 필터
- 선택한 요청의 Headers와 Body 상세 패널
- 프록시 시작, 중지, 기록 삭제
- 최대 2,000개의 메모리 기록
- 기록 큐 초과 건수 표시
- 프로그램 창 내부 전체를 PNG로 저장
- 화면 필터와 보관 한도에 관계없이 세션 전체 로그와 본문 저장(세션당 본문 512MiB)
- 캡처 범위와 기록되지 않는 트래픽 안내

## 제외 범위

현재 버전은 다음 기능을 제공하지 않는다.

- 시스템 전체 CA 등록과 기존 Firefox 개인 프로필의 인증서 저장소 변경
- Windows 지원
- 종료한 앱의 세션 복원과 SQLite 저장소
- 시스템 프록시 자동 변경
- HTTP/2와 HTTP/3
- WebSocket 프레임 분석
- Wayland 지원

HTTPS 복호화는 기본으로 꺼져 있다. 이 상태에서 앱은 `CONNECT` 터널 안의 URL, 헤더와 본문을 읽지 않는다. 사용자가 복호화를 켜면 앱은 HTTP/1.1 HTTPS 요청을 기존 상세 패널에 표시한다.

본문 미리보기는 메모리에 유지하며 화면 기록을 삭제하거나 앱을 종료하면 사라진다. 전체 로그는 앱이 실행되는 동안 별도의 임시 파일에 기록한다. `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie` 헤더의 원본 값은 저장 전에 `[REDACTED]`로 바꾸므로 메모리, 임시 로그, 저장한 로그 어디에도 남지 않는다. 큰 본문은 전체를 계속 전달하며 화면 미리보기만 8KiB에서 자른다.

요청·응답 본문의 `Content-Encoding`이 `gzip`, `deflate`, `br`, `zstd`이면 화면 미리보기에서 압축을 푼다. 압축 해제한 출력도 최대 8KiB로 제한하며, 캡처한 압축 데이터가 일부뿐이면 표시 가능한 앞부분과 불완전 상태를 보여 준다. 지원하지 않는 형식이나 손상된 압축 데이터에는 오류를 표시한다. 과도한 중첩 압축과 중간 데이터 확장은 미리보기를 거부한다. 프록시 전달과 전체 로그에는 압축된 원본 바이트를 유지한다.

한글 표시는 설치된 Nanum 또는 Noto CJK 폰트를 사용한다. Ubuntu에서 글자가 네모로 보이면 `sudo apt install fonts-noto-cjk`로 폰트를 설치하고 앱을 다시 실행한다.

다음은 Brotli로 압축한 테스트 JSON의 한글 본문을 미리보기에서 표시한 실제 화면이다.

![Brotli 압축을 푼 한글 JSON 본문 미리보기](docs/screenshots/decoded-body-preview.png)

다음은 Chrome으로 접속한 HTTPS 응답의 `Content-Encoding: br` 헤더와 압축을 푼 HTML 본문이다.

![HTTPS 응답의 Brotli 압축 해제와 HTML 본문 표시](docs/screenshots/decoded-https-body.png)

### PNG 응답 미리보기

`Content-Type`이 `image/png`인 응답은 **Body** 탭의 **Response body**에서 이미지로 그린다. 이미지를 그리려면 본문 전체가 필요하므로 PNG 응답만 응답당 최대 8MiB까지 메모리에 보관한다. 모든 PNG 응답이 함께 쓰는 보관 한도는 64MiB이며, 기록을 삭제하거나 오래된 기록이 밀려나면 그만큼 다시 쓸 수 있다. `Content-Length`가 8MiB를 넘는 PNG 응답은 공용 한도를 쓰지 않고 다른 본문처럼 앞 8KiB만 보관한다. 한도를 넘은 응답은 이미지 대신 보관한 바이트 수와 한도를 안내한다.

- `Content-Encoding`이 있으면 텍스트 미리보기와 같은 방식으로 압축을 푼 뒤 디코딩한다.
- 이미지 크기(가로×세로)와 PNG 바이트 수를 표시한다. 압축 전송이면 받은 바이트 수도 함께 표시한다.
- 가로·세로 8,192px 또는 16메가픽셀을 넘는 이미지는 헤더만 읽고 픽셀 메모리를 할당하기 전에 거부한다.
- 본문이 잘렸거나 손상되어 디코딩하지 못하면 오류를 표시한다. **Hex** 또는 **Show bytes as hex**로 받은 바이트의 앞 8KiB를 16진수로 확인할 수 있다.

다음은 gzip으로 전송한 테스트 PNG 응답을 이미지로 표시한 실제 화면이다.

![gzip 전송 PNG 응답을 이미지로 표시한 Response body](docs/screenshots/png-body-image.png)

다음은 본문 앞 3분의 1만 보낸 테스트 PNG 응답의 디코딩 오류와 16진수 보기다.

![잘린 PNG 응답의 디코딩 오류](docs/screenshots/png-body-error.png)

![잘린 PNG 응답의 16진수 보기](docs/screenshots/png-body-hex.png)

## Linux 요구사항

프로젝트는 Rust 1.100.0과 X11을 사용한다. `rustup`은 `rust-toolchain.toml`에 지정된 툴체인을 설치한다.

Ubuntu 계열 배포판에서 개발과 한글 표시에 필요한 패키지를 설치하려면 다음 명령을 사용한다.

```bash
sudo apt-get update
sudo apt-get install --yes \
	fonts-noto-cjk \
	libgl1-mesa-dev \
	libx11-dev \
	libxkbcommon-dev \
	libxkbcommon-x11-dev \
	libxcursor-dev \
	libxi-dev \
	libxrandr-dev \
	libnss3-tools \
	pkg-config
```

## 실행

저장소 루트에서 앱을 실행한다.

```bash
cargo run --release
```

UI에 `Running`과 `127.0.0.1:8080`이 표시되면 프록시가 요청을 받을 수 있다. 툴바의 `Copy` 메뉴는 프록시 URL, `curl` 예시, `HTTP_PROXY` 변수를 클립보드에 복사한다.

### 요청 목록 단축키

| 키 | 동작 |
| --- | --- |
| `↑`, `↓` | 이전 요청 또는 다음 요청을 선택합니다. |
| `Home`, `End` | 첫 요청 또는 마지막 요청을 선택합니다. |
| `PageUp`, `PageDown` | 목록에서 한 화면 위 또는 아래의 요청을 선택합니다. |
| `F6` | 선택한 요청의 **Headers**와 **Body** 탭을 전환합니다. |

목록 단축키는 현재 정렬과 필터를 따릅니다. 검색창, 포트 입력란 또는 저장 경로 입력란에 포커스가 있으면 단축키를 처리하지 않습니다. 키보드로 다른 요청을 선택하면 **Follow newest**가 꺼지고 선택한 행이 보이도록 목록이 스크롤됩니다.

### 화면 테마

상단의 **Theme: System / Light / Dark**에서 테마를 고른다. 기본값은 **System**이며, 감지 가능한 환경에서는 실행 중 시스템 외관 변경도 반영한다. 감지에 실패하면 Dark를 사용하고 화면에 안내한다. Tab으로 선택 항목에 이동한 뒤 Space로 바꿀 수 있다.

선택은 즉시 적용되며 포트 설정과 같은 디렉터리의 `theme` 파일에 저장된다.

- Linux: `$XDG_CONFIG_HOME/http-inspector/theme`. `XDG_CONFIG_HOME`이 없으면 `~/.config/http-inspector/theme`를 사용한다.
- macOS: `~/Library/Application Support/http-inspector/theme`.

설정 파일이 없거나 값이 잘못되면 System으로 시작한다.

### 화면 읽기 프로그램

AccessKit을 활성화해 요청 행과 기본 egui 컨트롤의 이름, 역할, 선택 상태를 접근성 트리에 제공합니다. Linux에서는 AT-SPI를 사용합니다. 요청 행은 메서드, 전체 URL, 상태를 함께 읽도록 이름을 제공합니다. HTTP 응답은 숫자와 이유 문구(예: `200 OK`, `404 Not Found`)를 표시합니다.

`Tab`과 `Shift+Tab`으로 요청 행과 상세 탭을 탐색하고 `Enter` 또는 `Space`로 선택합니다. 목록 단축키와 `F6`도 사용할 수 있습니다.

Linux에서 Orca를 실행한 뒤 테스트 요청을 보내고 요청 선택, 상태 읽기, **Headers**와 **Body** 전환을 점검하세요. 목록은 화면에 보이는 행만 접근성 트리에 제공하므로 화면 밖 요청은 목록 단축키로 이동하세요.

### 프록시 포트

프록시는 항상 `127.0.0.1`에서만 연결을 받는다. 포트는 다음 순서로 정한다.

1. `--port <번호>` 또는 `--port=<번호>` 명령줄 인자
2. `HTTP_INSPECTOR_PORT` 환경 변수
3. 툴바에서 마지막으로 입력해 시작에 성공한 포트(`$XDG_CONFIG_HOME/http-inspector/settings`, 없으면 `~/.config/http-inspector/settings`)
4. 기본값 `8080`

```bash
cargo run --release -- --port 18080
HTTP_INSPECTOR_PORT=0 cargo run --release
```

`0`은 운영체제가 비어 있는 포트를 고르게 한다. 실제 포트는 툴바 주소에 표시된다. 다른 프로그램이 포트를 쓰고 있으면 앱은 그 사실을 안내하고, 프록시를 멈춘 상태에서 `Port` 칸에 다른 번호를 입력해 `Start`로 다시 시작할 수 있다. 명령줄 인자와 환경 변수는 그 실행에만 적용되며 저장하지 않는다.

HTTP 요청을 확인하려면 `curl`에 프록시 주소를 지정한다.

```bash
curl --proxy http://127.0.0.1:8080 http://example.com/
```

환경 변수로 명령줄 프로그램에 프록시를 지정할 수도 있다.

```bash
HTTP_PROXY=http://127.0.0.1:8080 your-command
HTTPS_PROXY=http://127.0.0.1:8080 your-command
```

프로그램이 `NO_PROXY`를 설정하면 로컬 주소나 지정된 호스트가 프록시를 우회할 수 있다.

## 캡처 범위

앱은 클라이언트가 이 프록시로 보낸 HTTP 통신만 기록한다. 컴퓨터의 모든 트래픽을 수집하지 않는다. 화면 위쪽의 **What is captured?**를 펼치면 같은 안내를 볼 수 있다.

- HTTP는 메서드, URL, 헤더와 본문을 기록한다.
- **Decrypt HTTPS**를 끄면 HTTPS는 `Tunnel` 행으로 기록한다. 이 행에는 호스트, 포트, 전송 바이트 수와 연결 시간만 있다. URL 경로, 헤더와 본문은 암호화된 채 전달한다.
- **Decrypt HTTPS**를 켜고 클라이언트가 앱 CA를 신뢰하면 연결 안의 HTTP/1.1 요청을 하나씩 기록한다.

다음 트래픽은 기록되지 않는다.

- 프록시 설정을 무시하거나 직접 연결하는 프로그램의 통신
- `NO_PROXY`나 프록시 우회 목록에 포함된 호스트. 이 목록에는 흔히 `localhost`가 들어 있다.
- UDP를 사용하는 HTTP/3(QUIC). 브라우저는 첫 방문 뒤 HTTP/3으로 전환할 수 있다.
- VPN 터널이나 다른 프록시로 보낸 통신

**Open Browser**로 연 검사 브라우저는 QUIC을 끈다. 직접 실행하는 Chrome에는 [HTTPS 복호화](#https-복호화)의 예처럼 `--disable-quic`을 추가한다.

다음은 테스트용 HTTP 요청과 `CONNECT` 터널을 보낸 뒤 **What is captured?**를 펼치고 `Tunnel` 행을 선택한 실제 화면이다.

![캡처 범위 안내와 Tunnel 행 상세](docs/screenshots/capture-scope-help.png)

## 검사 브라우저 열기

1. **Browser** 목록에서 설치된 Firefox 또는 Chrome을 선택한다. Chromium도 선택할 수 있다. 앱은 기본 브라우저를 자동으로 선택하지 않는다.
2. HTTPS의 URL, 헤더와 본문을 확인하려면 프록시를 중지하고 **Decrypt HTTPS**를 선택한 뒤 **Start**를 누른다.
3. **Open Browser**를 누른다. 앱이 프록시를 설정한 검사 전용 프로필로 비공개 창을 연다. HTTPS 복호화를 켰으면 공개 CA도 자동으로 등록한다. Firefox에는 NSS `certutil`이 필요하다. Linux의 Chrome·Chromium도 NSS를 사용한다. macOS의 Chrome·Chromium은 로그인 키체인을 사용한다.
4. 열린 브라우저에서 확인할 사이트에 접속한다. HTTP Inspector의 요청 목록에서 트래픽을 확인한다.
5. 검사를 마치면 브라우저 창을 닫거나 **Close browser**를 누른다. **Stop**을 누르거나 앱을 종료해도 앱에서 연 검사 브라우저를 종료한다.

**Refresh browsers**는 `PATH`에서 Firefox, Firefox ESR, Google Chrome의 Stable·Beta·Dev, Chromium 실행 파일을 다시 찾는다. 같은 실행 파일의 별칭은 하나만 표시한다. Firefox Snap도 지원한다. Flatpak 브라우저와 Chromium Snap은 지원하지 않는다.

매번 새 검사 프로필을 만들며 브라우저 프로세스가 종료되면 삭제한다. 기존 개인 프로필의 쿠키와 로그인 상태는 가져오지 않는다. Firefox Snap의 임시 프로필은 `$HOME/snap/firefox/common` 아래에 만든다. 앱이 강제 종료되면 임시 프로필이 남을 수 있다. 비공개 창을 사용해도 HTTP Inspector의 요청 기록과 전체 로그는 남는다.

Firefox의 CA는 검사 전용 프로필에만 등록한다. Chrome·Chromium의 CA는 Linux에서 현재 사용자의 공용 NSS 저장소에, macOS에서 로그인 키체인에 등록하므로 개인 브라우저에도 신뢰 설정이 적용된다. Chrome·Chromium 등록은 검사 창을 닫아도 유지된다. 필요 없으면 **Decrypt HTTPS** 영역의 **Remove CA**로 제거한다.

다음은 실제 Firefox Snap에서 HTTPS를 복호화한 뒤 브라우저 선택 목록을 연 화면이다.

![Firefox와 Chrome 선택 목록과 복호화한 HTTPS 요청](docs/screenshots/browser-chooser.png)

검사 브라우저는 로컬 사이트도 프록시로 전달하고 HTTP/3을 비활성화한다. 인증서 검증은 유지한다. **Decrypt HTTPS**를 끄면 HTTPS는 `CONNECT` 터널로 전달하며 CA를 새로 등록하지 않는다.

## 창 캡처와 전체 로그 저장

1. **Save directory**에 저장할 기존 폴더의 경로를 입력한다. 기본값은 앱을 실행한 폴더다.
2. 창 이미지를 저장하려면 **Save window PNG**를 누른다. 현재 스크롤 위치를 포함한 앱 내부 전체가 PNG로 저장된다. 운영체제의 제목 표시줄과 테두리는 포함하지 않는다.
3. 전체 로그를 저장하려면 **Save all logs**를 누른다. 수집 시작부터 버튼을 누른 시점까지의 기록을 `.log` 텍스트 파일로 저장한다.
4. 화면에 표시된 **Saved:** 경로에서 파일을 확인한다. 저장할 때마다 새 파일을 만들며 기존 파일을 덮어쓰지 않는다.

다음 이미지는 테스트용 HTTP 트래픽을 보낸 뒤 **Save window PNG**로 저장한 실제 화면이다.

![창 캡처와 전체 로그 저장 버튼이 있는 HTTP Inspector 화면](docs/screenshots/save-window.png)

전체 로그에는 필터, 선택한 요청, 화면의 2,000건 보관 한도와 관계없이 모든 요청의 대상과 헤더, 응답 결과, 관찰한 요청·응답 본문, 트레일러를 기록한다. **Clear**는 화면 기록만 지운다. **Stop** 후 **Start**를 눌러도 같은 앱 세션의 전체 로그를 유지한다. 앱을 종료하면 임시 로그를 삭제하므로 필요한 로그는 종료 전에 저장한다.

로그는 발생 순서대로 기록하며 대괄호 안의 요청 번호로 내용을 구분한다. 본문 바이트는 `\n`, `\r`, `\t`, `\\`, `\xNN` 이스케이프를 사용한다. 같은 요청과 방향의 `body bytes:` 줄을 순서대로 합치고 이스케이프를 풀면 본문 바이트를 복원할 수 있다. UTF-8이 아닌 본문도 바이트를 잃지 않는다. 저장 시 수신 중인 본문은 그 시점까지 관찰한 내용이 들어가며, `body: complete` 또는 `body: interrupted`가 없는 방향은 아직 완료되지 않았을 수 있다.

전체 로그는 세션당 본문 바이트를 512MiB까지 기록한다. 한도에 도달하면 해당 본문 뒤에 `body: later bytes omitted ...` 줄을 남기고 이후 본문 바이트를 기록하지 않는다. 대상, 헤더와 결과는 계속 기록하며 툴바에 노란색 안내를 표시한다. 프록시는 본문 전체를 계속 전달한다.

복호화하지 않은 HTTPS는 CONNECT 연결 기록만 저장한다. 프록시가 본문을 읽기 전에 실패한 요청의 본문은 저장할 수 없다. 전체 로그에는 URL과 본문의 민감한 내용이 포함될 수 있으며, 위에 나열한 네 가지 헤더 외의 내용은 가리지 않는다. 임시 로그와 내보낸 파일은 소유자만 읽고 쓸 수 있는 권한으로 만든다.

디스크 쓰기 실패나 기록 대기열 초과가 발생하면 로그가 불완전하다는 오류를 표시하고 이후 로그 내보내기를 거부한다. 프록시의 트래픽 전달은 계속한다. 저장 위치 오류는 경로를 수정한 뒤 다시 저장할 수 있다.

## HTTPS 복호화

HTTPS 복호화는 사용 권한이 있는 트래픽에만 사용한다.

1. 실행 중인 프록시에서 **Stop**을 누른다.
2. **Decrypt HTTPS**를 선택한다.
3. UI에 표시된 `root-ca.pem` 경로와 SHA-256 지문을 확인한다. CA는 프록시를 시작하기 전에도 생성된다.
4. **Start**를 누른다.
5. **Browser**에서 브라우저를 선택한다.
6. **Open Browser**를 누른다. CA를 자동으로 등록한 뒤 검사 전용 비공개 창이 열린다.

기존 Chrome·Chromium을 직접 프록시에 연결하려면 **Register CA**로 CA를 등록한다. **CA is registered and trusted for HTTPS.**가 표시되는지 확인한다.

Linux에서 **Register CA**는 NSS의 `certutil` 명령으로 현재 사용자의 Chrome·Chromium 인증서 저장소에 공개 CA만 등록한다. 관리자 권한은 필요하지 않다. Ubuntu·Debian에서는 `sudo apt install libnss3-tools`, Fedora에서는 `sudo dnf install nss-tools`로 도구를 설치한다. 도구가 없거나 등록에 실패하면 화면에 오류를 표시한다. 수동으로 등록하려면 Chrome의 `chrome://settings/certificates`에서 공개 CA를 가져온다.

Linux의 등록 대상은 [Chromium의 NSS 저장소 선택 규칙](https://chromium.googlesource.com/chromium/src/+/HEAD/crypto/nss_util.cc)을 따른다. `$HOME/.pki/nssdb` 디렉터리가 있으면 그 경로를 사용한다. 없으면 `$XDG_DATA_HOME/pki/nssdb`를 사용하고, `XDG_DATA_HOME`이 없으면 `$HOME/.local/share/pki/nssdb`를 사용한다. 화면에 실제 대상 경로를 표시한다. 별도 저장소를 사용하는 Snap·Flatpak 브라우저와 Firefox, 시스템 전체 저장소에는 등록하지 않는다.

**Check registration**은 이 앱이 관리하는 항목의 인증서와 HTTPS 신뢰 설정을 확인한다. **Remove CA**는 현재 CA와 일치하는 앱 관리 항목만 제거한다. 다른 설치의 CA와 수동으로 등록한 다른 이름의 항목은 제거하지 않는다. 등록 상태를 바꾼 후에는 브라우저를 다시 시작한다.

기본 CA 경로는 다음과 같다.

```text
$XDG_DATA_HOME/http-inspector/ca/root-ca.pem
```

`XDG_DATA_HOME`이 없으면 다음 경로를 사용한다.

```text
$HOME/.local/share/http-inspector/ca/root-ca.pem
```

Chrome의 HTTP/3 우회를 막으려면 테스트 프로필을 다음과 같이 실행한다.

```bash
google-chrome \
	--user-data-dir=/tmp/http-inspector-chrome \
	--proxy-server=http://127.0.0.1:8080 \
	--disable-quic
```

Chrome에는 `root-ca.pem`만 등록한다. `ca-identity.pem`에는 개인키가 있으므로 열거나 복사하거나 신뢰 저장소에 등록하지 않는다. 앱을 삭제해도 등록한 CA는 남으므로, 삭제 전에 **Remove CA**로 제거한다. 수동으로 등록한 항목은 Chrome 설정에서 직접 제거한다.

## 검증

전체 로컬 검사를 실행한다.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo deny check
cargo build --release
```

통합 테스트는 실제 TCP 연결을 사용한다. 테스트는 HTTP 전달과 `CONNECT` 터널의 양방향 전송을 검증한다.

## 문서

- [네트워크 인스펙터 설계](docs/http-network-inspector-design.md)
- [Rust 개발 도구 및 의존성](docs/rust-development-tooling.md)
- [코드 품질 기준](docs/code-quality.md)
- [Git 작업 흐름](docs/git-workflow.md)
- [Network 상세 보기 설계](docs/f12-network-increment-design.md)
- [HTTPS 복호화 구현 설계](docs/https-inspection-implementation.md)
- [HTTPS 후속 작업](docs/https-future-work.md)

## macOS에서 실행하기

Apple Silicon과 Intel Mac에서 Rust 툴체인을 설치하고 다음 명령을 실행한다. X11은 필요하지 않다. 한글은 시스템의 Apple SD Gothic Neo 폰트를 사용한다.

```bash
cargo run --release
```

Firefox 검사 창에서 HTTPS를 복호화하려면 Homebrew NSS를 설치한다. Chrome 검사 창에는 NSS가 필요하지 않다.

```bash
brew install nss
```

**Open Browser**는 `/Applications`와 `~/Applications`에서 Firefox, Google Chrome, Chromium 앱 번들을 찾는다. 번들 안 실행 파일을 새 검사 프로필로 직접 실행한다. 기존 브라우저 창은 그대로 유지한다.

macOS에서는 CA를 `~/Library/Application Support/http-inspector/ca/`에, 포트 설정을 `~/Library/Application Support/http-inspector/settings`에 저장한다. `XDG_DATA_HOME`과 `XDG_CONFIG_HOME`은 사용하지 않는다. CA 디렉터리는 `0700`, 개인키 파일은 `0600` 권한으로 생성한다.

HTTPS를 복호화하려면 **Decrypt HTTPS**를 켜고 **Register CA**를 누른다. 앱은 `security add-trusted-cert`로 공개 CA를 로그인 키체인에 SSL 신뢰 대상으로 등록한다. macOS 인증 창이 나타나면 승인한다. 취소하면 화면에 오류가 표시되며 다시 시도할 수 있다. **Check registration**은 저장된 인증서 바이트와 SSL 신뢰 상태를 확인한다. **Remove CA**는 현재 CA의 SHA-256 지문과 일치하는 인증서 및 사용자 신뢰 설정을 제거한다. 변경 후 브라우저를 다시 실행한다. Safari도 로그인 키체인의 신뢰 설정을 사용하지만 검사 창 실행 목록에는 포함되지 않는다.

서명, 공증과 `.dmg` 배포는 지원하지 않는다. 시스템 키체인과 시스템 프록시 설정은 변경하지 않는다.

다음 화면은 Apple Silicon Mac에서 실행한 앱이 로컬 테스트 서버의 HTTP 응답을 기록한 모습이다. 앱의 **Save window PNG** 경로로 렌더링을 저장했다.

![macOS에서 로컬 테스트 HTTP 요청을 기록한 HTTP Inspector](docs/images/macos.png)

macOS 실환경에서 키체인 등록·상태 확인·제거와 Chrome 검사 창 실행을 확인하려면 다음 검사를 실행한다. 첫 검사는 임시 CA를 로그인 키체인에 등록한 뒤 제거하며, macOS 인증 창이 나타날 수 있다.

```bash
cargo test --test macos_integration -- --ignored
```

## 라이선스

이 프로젝트는 [MIT License](LICENSE)를 따른다.
