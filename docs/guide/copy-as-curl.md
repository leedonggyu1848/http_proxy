# 요청을 cURL 명령으로 복사하기

[사용자 가이드 목록](README.md) · [README](../../README.md)

요청 목록의 우클릭 메뉴나 상세 패널에서 **Copy as cURL**을 누르면 선택한 요청을
bash·zsh 같은 POSIX 셸에서 실행할 수 있는 `curl` 명령으로 클립보드에 복사한다.
복사가 끝나면 알림 줄에 결과가 표시된다.

## 명령에 들어가는 내용

- **메서드:** `curl`의 기본값과 다를 때만 `-X`를 쓴다. 본문이 없으면 `GET`, 본문이 있으면
  `POST`가 기본값이다. `HEAD`는 `--head`로 쓴다.
- **URL:** 캡처한 절대 URL을 그대로 쓴다. `[`·`]`·`{`·`}`가 있으면 `--globoff`를 붙인다.
- **헤더:** 클라이언트가 보낸 순서대로 `-H`로 넣는다. 값이 빈 헤더는 `-H 'Name;'`로 쓴다.
  프록시가 전달하지 않는 hop-by-hop 헤더(`Connection`, `Proxy-Connection`, `Keep-Alive`, `TE`,
  `Transfer-Encoding`, `Upgrade`, `Proxy-Authorization`과 `Connection`이 가리키는 헤더)와
  `curl`이 계산하는 `Content-Length`는 넣지 않는다. URL과 같은 `Host`도 생략한다.
- **curl 기본 헤더:** 원래 요청에 `User-Agent`·`Accept`가 없으면 `-H 'User-Agent:'`처럼 넣어
  `curl`이 자기 값을 추가하지 않게 한다. 본문이 있는데 `Content-Type`이 없을 때도 같다.
- **본문:** `--data-binary`로 원래 바이트를 그대로 보낸다. `Content-Encoding`으로 압축된 요청
  본문도 압축된 바이트 그대로 보낸다. 요청에 `Accept-Encoding`이 있으면 `--compressed`를 붙인다.

모든 값은 작은따옴표로 감싸므로 `$`, 백슬래시, 백틱, 줄바꿈, 한글이 셸에서 해석되지 않고
그대로 전달된다. 작은따옴표는 `'\''`로 바꾼다.

본문에 출력할 수 없는 바이트(NUL, 제어 문자, UTF-8이 아닌 바이트, CR)가 있거나 본문이 `@`로
시작하면, 본문을 `printf`의 8진수 이스케이프로 만들어 표준 입력으로 넘긴다.

```bash
printf '\037\213\000\001' | curl -X PUT 'http://127.0.0.1:8000/upload' \
  -H 'content-type: application/octet-stream' \
  --data-binary @-
```

`$'...'` 인용 대신 `printf`를 쓰는 이유는 `$'...'`를 지원하지 않는 `dash` 같은 `sh`에서도
실행되고, 셸 인자에 담을 수 없는 NUL 바이트도 보낼 수 있기 때문이다.

## 가려진 헤더

`Authorization`, `Cookie`의 원래 값은 저장되지 않는다(화면과 로그에는 `[REDACTED]`로 남는다).

- **기본값:** 이 헤더들을 명령에서 뺀다.
- **Redacted placeholders 켜기:** 상세 패널의 **Redacted placeholders**를 켜면
  `-H 'cookie: <REDACTED>'`처럼 자리 표시자로 넣는다. 실행 전에 실제 값으로 바꾼다.

어느 쪽이든 가려진 헤더가 있었다면 알림 줄에 해당 헤더 이름과 안내가 표시된다.
`Proxy-Authorization`은 프록시 전용 hop-by-hop 헤더이므로 어느 경우에도 넣지 않는다.

그 밖의 헤더와 본문은 캡처한 그대로 복사된다. 명령을 공유하기 전에 사용자 정의 헤더나 본문에
토큰 같은 민감정보가 있는지 확인한다.

## 복사하지 않는 경우

다음 기록은 실제 요청과 다른 명령이 만들어지므로 복사하지 않고 알림 줄에 이유를 표시한다.

- **잘린 본문:** 메모리에는 요청 본문 앞 8KiB만 남는다. 본문이 이보다 길면 잘린 명령을
  복사하지 않는다.
- 아직 전송 중이거나 중간에 끊긴 본문, 캡처하지 못한 본문
- 64개 또는 16KiB를 넘어 일부 헤더가 생략된 기록
- 4KiB를 넘어 잘린 URL
- UTF-8이 아닌 바이트가 있어 원래 값을 재현할 수 없는 헤더

`CONNECT` 터널(**Tunnel**) 기록은 HTTP 요청 내용이 없으므로 메뉴가 비활성화되고,
마우스를 올리면 이유가 표시된다.
