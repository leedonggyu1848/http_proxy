# 프록시 포트와 캡처 범위

[사용자 가이드 목록](README.md) · [README](../../README.md)

## 프록시 포트

프록시는 항상 `127.0.0.1`에서만 연결을 받는다. 포트는 다음 순서로 정한다.

1. `--port <번호>` 또는 `--port=<번호>` 명령줄 인자
2. `HTTP_INSPECTOR_PORT` 환경 변수
3. 툴바에서 마지막으로 입력해 시작에 성공한 포트(`$XDG_CONFIG_HOME/http-inspector/settings`, 없으면 `~/.config/http-inspector/settings`. macOS는 `~/Library/Application Support/http-inspector/settings`)
4. 기본값 `8080`

```bash
cargo run --release -- --port 18080
HTTP_INSPECTOR_PORT=0 cargo run --release
```

`0`은 운영체제가 비어 있는 포트를 고르게 한다. 실제 포트는 툴바 주소에 표시된다. 다른 프로그램이 포트를 쓰고 있으면 앱은 툴바 아래에 `Port 8080 is already in use; enter another port, or 0 for a free one.`처럼 한 줄로 안내한다. 프록시가 멈춘 동안 툴바의 `Port` 칸에 다른 번호를 입력하고 오른쪽의 **Start**를 눌러 다시 시작한다. 명령줄 인자와 환경 변수는 그 실행에만 적용되며 저장하지 않는다.

HTTP 요청을 확인하려면 `curl`에 프록시 주소를 지정한다.

```bash
curl --proxy http://127.0.0.1:8080 http://example.com/
```

환경 변수로 명령줄 프로그램에 프록시를 지정할 수도 있다.

```bash
http_proxy=http://127.0.0.1:8080 https_proxy=http://127.0.0.1:8080 curl http://example.com/
```

프로그램이 `NO_PROXY`를 설정하면 로컬 주소나 지정된 호스트가 프록시를 우회할 수 있다.

## 캡처 범위

앱은 클라이언트가 이 프록시로 보낸 HTTP 통신만 기록한다. 컴퓨터의 모든 트래픽을 수집하지 않는다. 툴바 **Settings** 메뉴의 **Capture scope** 하위 메뉴에서 같은 안내를 볼 수 있다.

- HTTP는 메서드, URL, 헤더와 본문을 기록한다.
- **Decrypt HTTPS**를 끄면 HTTPS는 `Tunnel` 행으로 기록한다. 이 행에는 호스트, 포트, 전송 바이트 수와 연결 시간만 있다. URL 경로, 헤더와 본문은 암호화된 채 전달한다.
- **Decrypt HTTPS**를 켜고 클라이언트가 앱 CA를 신뢰하면 연결 안의 HTTP/1.1 요청을 하나씩 기록한다.

다음 트래픽은 기록되지 않는다.

- 프록시 설정을 무시하거나 직접 연결하는 프로그램의 통신
- `NO_PROXY`나 프록시 우회 목록에 포함된 호스트. 이 목록에는 흔히 `localhost`가 들어 있다.
- UDP를 사용하는 HTTP/3(QUIC). 브라우저는 첫 방문 뒤 HTTP/3으로 전환할 수 있다.
- 이 프록시를 거치지 않고 VPN이나 다른 프록시로 직접 보낸 통신

**Open Browser**로 연 검사 브라우저는 QUIC을 끈다. 직접 실행하는 Chrome에는 [HTTPS 복호화와 검사 브라우저](https.md#직접-실행한-chrome에서-http3-막기)의 예처럼 `--disable-quic`을 추가한다.

다음은 테스트용 HTTP 요청과 `CONNECT` 터널을 보낸 뒤 `Tunnel` 행을 선택하고 **Settings** 메뉴의 **Capture scope**를 펼친 실제 화면이다.

![캡처 범위 안내와 Tunnel 행 상세](../screenshots/capture-scope-help.png)
