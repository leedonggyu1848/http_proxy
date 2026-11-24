# HTTPS 복호화와 검사 브라우저

[사용자 가이드 목록](README.md) · [README](../../README.md)

HTTPS 복호화는 기본으로 꺼져 있다. 이 상태에서 앱은 `CONNECT` 터널 안의 URL, 헤더와 본문을 읽지 않는다. 사용자가 복호화를 켜면 앱은 HTTP/1.1 HTTPS 요청을 기존 상세 패널에 표시한다.

## HTTPS 복호화 켜기

HTTPS 복호화는 사용 권한이 있는 트래픽에만 사용한다.

1. 실행 중인 프록시에서 **Stop**을 누른다.
2. **Decrypt HTTPS**를 선택한다.
3. 툴바에 CA 상태가 표시된다. **CA details** 메뉴에서 `root-ca.pem` 경로와 SHA-256 지문을 확인하고 복사할 수 있다. CA는 프록시를 시작하기 전에도 생성된다.
4. **Start**를 누른다.
5. **Open Browser** 메뉴에서 브라우저를 누른다. CA를 자동으로 등록한 뒤 검사 전용 비공개 창이 열린다.

처음 **Decrypt HTTPS**를 켜면 권한이 있는 트래픽에만 사용하라는 경고를 한 번 표시한다. 같은 안내는 체크박스 툴팁에서도 볼 수 있다.

툴바의 CA 상태는 색과 짧은 문구로 표시한다.

| 표시 | 의미 | 툴바 동작 |
| --- | --- | --- |
| `● Trusted` (초록) | 등록되어 HTTPS 신뢰 대상이다. | **Remove CA** |
| `● Not registered` (노랑) | 이 앱이 등록한 항목이 없다. | **Register CA** |
| `● Registered, not trusted` (빨강) | 등록되었지만 HTTPS 신뢰 설정이 없다. | **Register CA** 재시도 |
| `● Error` (빨강) | 확인 또는 등록에 실패했다. 오류는 툴바 아래에 표시한다. | **Register CA** |
| 스피너 | 확인·등록·제거 중이다. | 없음 |

## 검사 브라우저 열기

1. HTTPS의 URL, 헤더와 본문을 확인하려면 프록시를 중지하고 **Decrypt HTTPS**를 선택한 뒤 **Start**를 누른다.
2. 툴바의 **Open Browser** 메뉴에서 설치된 Firefox, Chrome 또는 Chromium을 누른다. 앱은 기본 브라우저를 자동으로 선택하지 않는다. 각 항목에 마우스를 올리면 CA 등록 방식을 안내한다.
3. 앱이 프록시를 설정한 검사 전용 프로필로 비공개 창을 연다. HTTPS 복호화를 켰으면 공개 CA도 자동으로 등록한다. Firefox에는 NSS `certutil`이 필요하다. Linux의 Chrome·Chromium도 NSS를 사용한다. macOS의 Chrome·Chromium은 로그인 키체인을 사용한다.
4. 열린 브라우저에서 확인할 사이트에 접속한다. HTTP Inspector의 요청 목록에서 트래픽을 확인한다.
5. 검사를 마치면 브라우저 창을 닫거나 **Close browser**를 누른다. **Stop**을 누르거나 앱을 종료해도 앱에서 연 검사 브라우저를 종료한다.

**Open Browser** 메뉴의 **Refresh browsers**는 `PATH`에서 Firefox, Firefox ESR, Google Chrome의 Stable·Beta·Dev, Chromium 실행 파일을 다시 찾는다. macOS에서는 `/Applications`와 `~/Applications`의 브라우저 앱도 찾는다. 같은 실행 파일의 별칭은 하나만 표시한다. Firefox Snap도 지원한다. Flatpak 브라우저와 Chromium Snap은 지원하지 않는다.

매번 새 검사 프로필을 만들며 브라우저 프로세스가 종료되면 삭제한다. 기존 개인 프로필의 쿠키와 로그인 상태는 가져오지 않는다. Firefox Snap의 임시 프로필은 `$HOME/snap/firefox/common` 아래에 만든다. 앱이 강제 종료되면 임시 프로필이 남을 수 있다. 비공개 창을 사용해도 HTTP Inspector의 요청 기록과 전체 로그는 남는다.

Firefox의 CA는 검사 전용 프로필에만 등록한다. Chrome·Chromium의 CA는 Linux에서 현재 사용자의 공용 NSS 저장소에, macOS에서 로그인 키체인에 등록하므로 개인 브라우저에도 신뢰 설정이 적용된다. Chrome·Chromium 등록은 검사 창을 닫아도 유지된다. 필요 없으면 툴바의 **Remove CA** 또는 **CA details** 메뉴의 **Remove CA**로 제거한다.

다음은 실제 Firefox Snap에서 HTTPS를 복호화한 뒤 브라우저 선택 목록을 연 화면이다. 이 캡처는 브라우저 선택이 툴바 메뉴로 옮겨지기 전의 화면이다.

![Firefox와 Chrome 선택 목록과 복호화한 HTTPS 요청](../screenshots/browser-chooser.png)

검사 브라우저는 로컬 사이트도 프록시로 전달하고 HTTP/3을 비활성화한다. 인증서 검증은 유지한다. **Decrypt HTTPS**를 끄면 HTTPS는 `CONNECT` 터널로 전달하며 CA를 새로 등록하지 않는다.

## 기존 Chrome·Chromium에 CA 등록

기존 Chrome·Chromium을 직접 프록시에 연결하려면 **Register CA**로 CA를 등록한다. CA 상태가 `● Trusted`로 바뀌는지 확인한다.

### Linux

Linux에서 **Register CA**는 NSS의 `certutil` 명령으로 현재 사용자의 Chrome·Chromium 인증서 저장소에 공개 CA만 등록한다. 관리자 권한은 필요하지 않다. Ubuntu·Debian에서는 `sudo apt install libnss3-tools`, Fedora에서는 `sudo dnf install nss-tools`로 도구를 설치한다. 도구가 없거나 등록에 실패하면 화면에 오류를 표시한다. 수동으로 등록하려면 Chrome의 `chrome://settings/certificates`에서 공개 CA를 가져온다.

Linux의 등록 대상은 [Chromium의 NSS 저장소 선택 규칙](https://chromium.googlesource.com/chromium/src/+/HEAD/crypto/nss_util.cc)을 따른다. `$HOME/.pki/nssdb` 디렉터리가 있으면 그 경로를 사용한다. 없으면 `$XDG_DATA_HOME/pki/nssdb`를 사용하고, `XDG_DATA_HOME`이 없으면 `$HOME/.local/share/pki/nssdb`를 사용한다. **CA details** 메뉴에 실제 대상 경로를 표시한다. 별도 저장소를 사용하는 Snap·Flatpak 브라우저와 Firefox, 시스템 전체 저장소에는 등록하지 않는다.

**CA details** 메뉴의 **Check registration** 또는 CA 상태 문구 클릭은 이 앱이 관리하는 항목의 인증서와 HTTPS 신뢰 설정을 확인한다. **Remove CA**는 현재 CA와 일치하는 앱 관리 항목만 제거한다. 다른 설치의 CA와 수동으로 등록한 다른 이름의 항목은 제거하지 않는다. 등록 상태를 바꾼 후에는 브라우저를 다시 시작한다.

기본 CA 경로는 다음과 같다.

```text
$XDG_DATA_HOME/http-inspector/ca/root-ca.pem
```

`XDG_DATA_HOME`이 없으면 다음 경로를 사용한다.

```text
$HOME/.local/share/http-inspector/ca/root-ca.pem
```

### macOS

HTTPS를 복호화하려면 **Decrypt HTTPS**를 켜고 **Register CA**를 누른다. 앱은 `security add-trusted-cert`로 공개 CA를 로그인 키체인에 SSL 신뢰 대상으로 등록한다. macOS 인증 창이 나타나면 승인한다. 취소하면 화면에 오류가 표시되며 다시 시도할 수 있다. **Check registration**은 저장된 인증서 바이트와 SSL 신뢰 상태를 확인한다. **Remove CA**는 현재 CA의 SHA-256 지문과 일치하는 인증서 및 사용자 신뢰 설정을 제거한다. 변경 후 브라우저를 다시 실행한다. Safari도 로그인 키체인의 신뢰 설정을 사용하지만 검사 창 실행 목록에는 포함되지 않는다.

macOS에서는 CA를 `~/Library/Application Support/http-inspector/ca/`에 저장한다. `XDG_DATA_HOME`은 사용하지 않는다. CA 디렉터리는 `0700`, 개인키 파일은 `0600` 권한으로 생성한다.

**Open Browser**는 앱 번들 안 실행 파일을 새 검사 프로필로 직접 실행한다. 기존 브라우저 창은 그대로 유지한다. Firefox 검사 창에서 HTTPS를 복호화하려면 Homebrew NSS(`brew install nss`)가 필요하다. Chrome 검사 창에는 NSS가 필요하지 않다.

서명, 공증과 `.dmg` 배포는 지원하지 않는다. 시스템 키체인과 시스템 프록시 설정은 변경하지 않는다.

## 직접 실행한 Chrome에서 HTTP/3 막기

Chrome의 HTTP/3 우회를 막으려면 테스트 프로필을 다음과 같이 실행한다.

```bash
google-chrome \
	--user-data-dir=/tmp/http-inspector-chrome \
	--proxy-server=http://127.0.0.1:8080 \
	--disable-quic
```

## 개인키와 CA 제거

Chrome에는 `root-ca.pem`만 등록한다. `ca-identity.pem`에는 개인키가 있으므로 열거나 복사하거나 신뢰 저장소에 등록하지 않는다. 앱을 삭제해도 등록한 CA는 남으므로, 삭제 전에 **Remove CA**로 제거한다. 수동으로 등록한 항목은 Chrome 설정에서 직접 제거한다.
