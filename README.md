# HTTP Inspector

HTTP Inspector는 로컬 HTTP 프록시를 통해 요청·응답 기록을 보여 주는 Linux와 macOS 데스크톱 앱이다. 원하면 HTTPS도 복호화해 URL, 헤더와 본문을 확인할 수 있다. UI와 프록시 엔진은 모두 Rust로 작성했다.

![HTTPS 검사를 켠 상태에서 테스트 요청 21건을 기록한 화면](docs/screenshots/theme-dark-ca.png)

## 주요 기능

- **로컬 프록시:** `127.0.0.1`에서만 연결을 받는 HTTP/1.1 프록시(기본 포트 `8080`). HTTPS는 기본적으로 `CONNECT` 터널로 전달한다.
- **선택적 HTTPS 복호화:** 설치별 로컬 CA로 HTTPS를 복호화한다. 실제 서버 인증서와 호스트 이름은 그대로 검증한다.
- **검사 브라우저:** 설치된 Firefox·Chrome·Chromium을 프록시와 CA가 설정된 비공개 창으로 연다.
- **요청 목록과 상세:** 메서드, 대상, 상태, 시간을 목록으로 보고, 선택한 요청의 헤더와 본문을 확인한다. 검색과 결과 필터를 제공한다.
- **본문 보기:** 압축을 푼 본문을 JSON·HTML·XML·CSS·JavaScript·form 형식별로 정리해 보여 주고, 이미지 응답은 그림으로 그린다.
- **요청 재전송:** 기록한 요청을 수정해 다시 보내고 원본과 비교한다.
- **저장:** 앱 창을 PNG로, 세션의 모든 요청을 텍스트 로그로 저장한다.

전체 기능 목록과 세부 동작은 [사용자 가이드](docs/guide/README.md)에 있다.

## 설치

소스에서 빌드해 실행한다. 먼저 [rustup](https://rustup.rs/)을 설치한다. `rustup`은 저장소의 `rust-toolchain.toml`에 지정된 Rust 툴체인(1.100.0)을 설치한다.

| | Linux | macOS |
| --- | --- | --- |
| 지원 환경 | X11 (Wayland 미지원) | Apple Silicon, Intel |
| 필수 패키지 | 아래 `apt-get` 명령 | 없음 |
| HTTPS CA 등록 도구 | `libnss3-tools`(NSS `certutil`, 필수 패키지에 포함) | Firefox 검사 창에만 `brew install nss` |
| 한자·가나 표시 | 선택: `fonts-noto-cjk` | 시스템의 Apple SD Gothic Neo 사용 |

한글은 앱에 내장한 폰트로 표시하므로 폰트 패키지가 필요하지 않다.

**Linux (Ubuntu 계열)**

```bash
sudo apt-get update
sudo apt-get install --yes \
	libgl1-mesa-dev \
	libx11-dev \
	libxkbcommon-dev \
	libxkbcommon-x11-dev \
	libxcursor-dev \
	libxi-dev \
	libxrandr-dev \
	libnss3-tools \
	pkg-config
# 선택: 한자·가나 표시
sudo apt-get install --yes fonts-noto-cjk
```

Fedora에서는 NSS 도구를 `sudo dnf install nss-tools`로 설치한다.

**macOS**

```bash
# 선택: Firefox 검사 창에서 HTTPS를 복호화할 때만 필요
brew install nss
```

## 빠른 시작

1. 저장소 루트에서 앱을 실행한다.

   ```bash
   cargo run --release
   ```

2. 앱을 실행하면 프록시가 시작된다. 툴바에 `● Running`과 `127.0.0.1:8080`이 표시되면 요청을 받을 수 있다. 멈춰 있으면 **Start**를 누른다.
3. 툴바의 **Open Browser** 메뉴에서 Firefox, Chrome 또는 Chromium을 누른다. 프록시가 설정된 검사 전용 비공개 창이 열린다.
4. 열린 창에서 확인할 사이트에 접속한다. 요청이 앱의 목록에 나타나며, 행을 선택하면 **Headers**와 **Body** 탭에서 상세를 본다.

명령줄 프로그램은 프록시 주소를 직접 지정한다. 툴바의 `Copy` 메뉴로 프록시 URL, `curl` 예시, `HTTP_PROXY` 변수를 복사할 수 있다.

```bash
curl --proxy http://127.0.0.1:8080 http://example.com/
HTTP_PROXY=http://127.0.0.1:8080 HTTPS_PROXY=http://127.0.0.1:8080 your-command
```

HTTPS 복호화를 켜지 않으면 HTTPS는 호스트·포트·바이트 수만 담은 `Tunnel` 행으로 기록된다.

## HTTPS 복호화

HTTPS 복호화는 기본으로 꺼져 있으며, 사용 권한이 있는 트래픽에만 사용한다.

1. 실행 중인 프록시에서 **Stop**을 누른다.
2. **Decrypt HTTPS**를 선택한다. 처음 켜면 경고를 한 번 표시하고, 툴바에 CA 상태가 나타난다.
3. **Start**를 누른다.
4. **Open Browser** 메뉴에서 브라우저를 누른다. 앱이 공개 CA를 자동으로 등록한 뒤 검사 창을 연다.

검사 창 대신 기존 Chrome·Chromium을 직접 프록시에 연결하려면 툴바의 **Register CA**를 누른다. Linux는 현재 사용자의 NSS 저장소에, macOS는 로그인 키체인에 공개 CA를 등록한다(macOS는 인증 창에서 승인한다).

- **확인:** CA 상태가 `● Trusted`(초록)로 바뀌는지 본다. **CA details** 메뉴에서 `root-ca.pem` 경로와 SHA-256 지문을 확인하고 **Check registration**으로 다시 검사한다.
- **제거:** 툴바 또는 **CA details** 메뉴의 **Remove CA**를 누른다. Chrome·Chromium 등록은 검사 창을 닫아도 유지되며 개인 브라우저에도 적용되므로, 필요 없으면 제거한다. 등록 상태를 바꾼 뒤에는 브라우저를 다시 시작한다.

> **보안 주의**
>
> - 신뢰 저장소에는 공개 인증서 `root-ca.pem`만 등록한다. 같은 폴더의 `ca-identity.pem`에는 CA 개인키가 있으므로 열거나 복사하거나 공유하지 않는다.
> - 앱을 삭제해도 등록한 CA는 남는다. 삭제 전에 **Remove CA**로 제거한다. 수동으로 등록한 항목은 브라우저 설정에서 직접 제거한다.
> - `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie` 헤더 값은 기록 전에 `[REDACTED]`로 바뀌어 화면과 로그에 남지 않는다. URL, 다른 헤더와 본문은 가리지 않으므로 저장한 로그를 공유할 때 주의한다.

CA 상태 표시의 의미, OS별 CA 저장 위치, 직접 실행한 Chrome 설정은 [HTTPS 복호화와 검사 브라우저](docs/guide/https.md)를 참고한다.

## 자주 쓰는 기능

- **필터·검색:** 필터 줄에 검색어를 입력하고 종류(All·Responses·Failures·Tunnels)를 고른다. **Follow newest**는 새 요청을 따라가고 **Clear**는 화면 기록을 지운다.
- **본문 보기:** **Body** 탭의 **View: Pretty / Raw / Tree**로 보기 방식을 바꾼다. 미리보기는 방향별 최대 8KiB다. 자세한 내용은 [본문 보기](docs/guide/body-view.md)를 참고한다.
- **요청 재전송:** 요청을 우클릭하거나 상세 패널에서 **Edit and Resend**를 눌러 수정한 뒤 **Send**한다. [요청을 수정해 다시 보내기](docs/guide/resend.md)를 참고한다.
- **로그·창 저장:** 툴바의 **Save** 메뉴에서 저장 폴더를 정하고 **Save window PNG** 또는 **Save all logs**를 누른다. 전체 로그는 앱을 종료하면 삭제되므로 종료 전에 저장한다. [창 캡처와 전체 로그 저장](docs/guide/logs.md)을 참고한다.
- **포트 변경:** 프록시를 멈추고 툴바의 `Port` 칸에 번호를 입력해 **Start**를 누르거나, `cargo run --release -- --port 18080` 또는 `HTTP_INSPECTOR_PORT`로 실행한다. [프록시 포트와 캡처 범위](docs/guide/proxy-and-capture.md)를 참고한다.
- **단축키:** 요청 목록에서 `↑`·`↓`·`Home`·`End`·`PageUp`·`PageDown`으로 요청을 고르고, `F6` 또는 `Ctrl+PageDown`·`Ctrl+PageUp`으로 **Headers**·**Body** 탭을 바꾼다. 전체 표와 테마·접근성 설정은 [화면과 조작](docs/guide/interface.md)에 있다.

## 제한 사항

다음 트래픽은 기록되지 않는다. 툴바의 **?** 버튼에서도 같은 안내를 볼 수 있다.

- 프록시 설정을 무시하거나 직접 연결하는 프로그램의 통신
- `NO_PROXY`나 프록시 우회 목록에 포함된 호스트(흔히 `localhost` 포함)
- UDP를 사용하는 HTTP/3(QUIC). **Open Browser**로 연 검사 브라우저는 QUIC을 끈다.
- VPN 터널이나 다른 프록시로 보낸 통신

현재 버전은 다음 기능을 제공하지 않는다.

- 시스템 전체 CA 등록과 기존 Firefox 개인 프로필의 인증서 저장소 변경
- Windows 지원, Wayland 지원
- HTTP/2와 HTTP/3, WebSocket 프레임 분석
- 시스템 프록시 자동 변경
- 종료한 앱의 세션 복원과 SQLite 저장소
- macOS 앱 서명, 공증과 `.dmg` 배포

## 문서

- [사용자 가이드](docs/guide/README.md): 기능별 세부 동작과 한도
- [개발 문서](docs/development.md): 개발 환경, 검증 명령, 설계 문서

## 라이선스

이 프로젝트의 코드는 [MIT License](LICENSE)를 따른다.

앱에 내장한 한글 폰트 `assets/fonts/NotoSansKR-Regular-subset.otf`는 Noto Sans KR의 서브셋이며 코드와 별개로 [SIL Open Font License 1.1](assets/fonts/OFL.txt)을 따른다. 출처와 재생성 방법은 [assets/fonts/README.md](assets/fonts/README.md)에 있다.
