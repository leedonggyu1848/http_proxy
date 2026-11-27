---
name: http-inspector-cli
description: "HTTP Inspector 명령줄(`http-proxy`)로 로컬 프록시를 띄우고 HTTP 요청·응답을 캡처해 읽고, 고쳐서 재전송하고, HAR·OpenAPI로 내보낸다. 앱이나 서버가 실제로 보내고 받는 HTTP를 확인해야 할 때, 요청을 바꿔 다시 보내 결과를 비교할 때, 캡처한 API의 구조가 필요할 때 사용한다."
---

# HTTP Inspector 명령줄

`http-proxy`는 로컬 프록시(`127.0.0.1`)를 거치는 HTTP 요청·응답을 기록한다. 하위 명령을 주면 디스플레이 없이 명령줄로 동작한다. 전체 명령·필드·오류 코드는 [docs/cli.md](../../../docs/cli.md) 또는 `http-proxy schema`에 있다.

## 규칙

- 모든 명령에 `--json`을 붙이고 JSON을 읽는다. 결과와 오류 모두 stdout에 한 줄 JSON으로 나오며 최상위에 `"schema": 1`이 있다. 필드는 이름으로 읽는다.
- 실패하면 종료 코드와 `error.code`, `error.hint`로 다음 행동을 정한다. 추측하지 않는다.

  | 종료 코드 | 뜻 | 다음 행동 |
  | --- | --- | --- |
  | `0` | 성공 | |
  | `2` | 잘못된 인자·필터 | `--help`, 필터 문법 확인 |
  | `3` | 데몬 없음 | `http-proxy proxy start --port 0 --json` |
  | `4` | 기록·본문·트리 경로 없음 | ID는 `records list`, 경로는 `records body ID --view tree`로 확인 |
  | `5` | `--yes` 필요 | `--dry-run`으로 확인한 뒤 `--yes` |
  | `6` | 대기 시간 초과 | 요청이 프록시를 거쳤는지, 필터가 맞는지 확인 |
  | `75` | 일시적으로 바쁨 | 잠시 뒤 다시 실행 |
  | `1` | 그 밖의 실패 | `error.code`와 `error.hint`를 따른다 |

- 어떤 명령도 입력을 기다리지 않는다. 서버 상태를 바꿀 수 있는 재전송만 `--yes`를 요구한다.
- `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie` 값은 캡처할 때 `[REDACTED]`로 바뀐다. 원래 값은 어디에도 없으므로 재전송에 필요하면 직접 `--header`로 준다.
- 기록 ID는 정수이며 목록·상세·본문·재전송이 같은 ID를 쓴다.

## 시작과 정리

```bash
http-proxy proxy start --port 0 --json      # .address = 프록시 주소, 이미 떠 있으면 already_running: true
http-proxy exec -- curl -sS …               # 한 명령만 프록시·CA 설정으로 실행. 종료 코드는 명령의 것
eval "$(http-proxy proxy env)"              # 이미 열린 셸 전체를 프록시로 보낼 때

http-proxy proxy status --json              # 실행 여부, 주소, 기록 수(데몬이 없으면 종료 코드 3)
http-proxy log save ./session.log --json    # 필요하면 끝내기 전에 전체 로그 저장
http-proxy proxy stop --json                # 데몬 종료. 보관한 기록은 사라진다
```

HTTPS 본문까지 봐야 하면 `proxy start --https-inspection`(이미 떠 있으면 `proxy set --https-inspection on`)을 쓴다. `exec`는 시스템 루트와 로컬 CA를 합친 임시 CA 묶음을 `SSL_CERT_FILE` 등에 지정하므로 `--cacert` 없이 curl·Python·git·Node가 로컬 CA를 믿고, 명령이 끝나면 묶음을 지운다. `exec`는 대화형 셸이 필요한 `http-proxy shell`과 달리 입력을 기다리지 않는다.

## 흐름 1: 요청을 보내고 응답 확인

```bash
curl -s http://127.0.0.1:3000/api/users > /dev/null          # 프록시를 거치는 요청
http-proxy records wait --filter 'method:GET url:/api/users' --timeout 10s --json
#   → .record.id, .record.status. 이미 끝난 요청도 찾는다
http-proxy records show <ID> --json                           # 헤더, 쿼리, 결과, 시간
http-proxy records body <ID> --json --max-bytes 4096          # 응답 본문(.text, 기본은 들여쓴 Pretty)
http-proxy records body <ID> --view raw --json                # 압축만 푼 원래 텍스트
http-proxy records body <ID> --json-path '$.data[0].id' --json   # JSON 값 하나(.text)
http-proxy records body <ID> --view tree --json               # 경로 목록(.nodes[].path)
```

- `--json-path`의 `.text`는 그 값의 JSON 원문이다. 문자열은 따옴표가 붙어 `"\"pro\""`처럼 나오므로 `jq -r '.text | fromjson'`으로 값을 꺼낸다.
- 경로는 Tree 보기와 같은 형식이다. `$.echo.x-client`처럼 쓰면 `$.echo["x-client"]`로 읽으므로 하이픈이 든 키도 점으로 써도 된다. 키에 `.`이 있으면 `$["a.b"]`처럼 괄호로 쓴다.
- 필터는 앱과 같은 문법이다: `method:POST status:4xx host:api.test url:/login -path:/static "phrase"`. 쓸 수 없는 조건이 있으면 종료 코드 2로 실패한다.
- `records list --json`은 요약(`id, method, url, outcome, status, duration_ms, request_bytes, response_bytes`)만 준다. 모든 필드는 `--full` 또는 `records show`.
- 이진 본문은 `binary_body`로 실패한다. `--view hex`로 본다.

## 흐름 2: 고쳐서 재전송

```bash
http-proxy records replay <ID> --header 'X-Debug: 1' --dry-run --json   # 보낼 요청만 확인(.request)
http-proxy records replay <ID> --header 'X-Debug: 1' --json             # 보내고 새 기록(.record) 출력
http-proxy records replay <ID> --method POST --body-file body.json --yes --json
http-proxy records curl <ID> --json                                    # 같은 요청의 curl 명령(.command)
```

- `--header 'Name: value'`는 같은 이름의 캡처 헤더를 모두 **대체**한다(추가하지 않는다). `--remove-header Name`은 뺀다.
- 원래 요청에 가려진 헤더가 있으면 `not_reproducible`로 실패하고 이름을 알려 준다. `--header 'Authorization: Bearer …'`로 값을 주거나 `--remove-header Authorization`으로 뺀다.
- `GET`·`HEAD`·`OPTIONS`가 아닌 메서드는 `--yes`가 없으면 보내지 않는다(종료 코드 5). 먼저 `--dry-run`으로 확인한다.
- 앱의 **Edit and Resend**와 같이 `Host`와 `Content-Length`를 다시 계산해 보낸다. 본문 없는 `GET`에도 `content-length: 0`이 붙는다. hop-by-hop 헤더와 `Expect`는 보내지 않는다.
- 새 기록의 `replay_of`가 원본 ID다. 서버가 받은 내용은 새 기록의 `records show`·`records body`로 확인한다.

## 흐름 3: HAR와 API 구조

```bash
http-proxy records export --filter 'host:api.test' --output api.har --json   # HAR 1.2 파일(덮어쓰지 않음)
http-proxy records endpoints --filter 'host:api.test' --json                 # 엔드포인트별 필드·형식·횟수
http-proxy records endpoints --filter 'host:api.test' --format openapi > openapi.json   # OpenAPI 3.1
```

- 필터를 생략하면 보관한 기록 전체를 대상으로 한다.
- `--output` 경로에 파일이 있으면 덮어쓰지 않고 `file_exists`(종료 코드 1)로 실패한다. 새 이름을 쓴다.
- `--output`이 없는 `records export`와 `--format openapi`는 문서 자체를 stdout에 쓴다.
- OpenAPI는 호스트 하나만 표현한다. 여러 호스트가 섞이면 `--filter host:…`로 좁힌다.

## 흐름 4: HLS 영상 저장

```bash
http-proxy records list --filter 'url:m3u8' --json        # 재생목록 기록 ID
http-proxy records hls <ID> --json                        # 화질 목록과 받을 수 있는지(.variants[].can_download, .blocked)
http-proxy records hls <ID> --variant 1 --output video.ts --json   # NDJSON progress … completed
```

현재 세션에 그 화질의 Chrome `.ts` 세그먼트 요청이 캡처되어 있어야 받을 수 있다. 막히면 `.blocked`가 이유를 알려 준다.

## 하지 않는 일

검사 브라우저 열기, CA를 브라우저·시스템에 등록, URL 맵 보기는 앱에서만 한다. 필요하면 사용자에게 앱을 쓰도록 안내한다.
