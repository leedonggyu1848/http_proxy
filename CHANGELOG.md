# 변경 기록

이 파일은 사용자가 체감하는 변경을 버전별로 기록한다. 형식은 [Keep a Changelog 1.1.0](https://keepachangelog.com/ko/1.1.0/)을, 버전 번호는 [유의적 버전(SemVer) 2.0.0](https://semver.org/lang/ko/)을 따른다. 작성 규칙과 릴리스 절차는 [개발 문서의 릴리스와 버전](docs/development.md#릴리스와-버전)에 있다.

## [Unreleased]

### 추가

- Linux x86_64용 `.deb` 패키지(`http-inspector`)를 배포한다. 실행 파일을 `/usr/bin/http-proxy`에 설치하고 애플리케이션 메뉴 항목과 아이콘을 등록한다.
- 첫 공개 버전에 포함될 기능: HTTP/1.1 요청·응답 스트리밍 기록, 선택적 HTTPS 복호화와 검사 브라우저, 요청 재전송과 cURL 복사, URL 맵과 관계 그래프, 로그·본문·HAR 1.2 저장.

### 수정

- 프록시를 중지할 때 열려 있던 `CONNECT` 터널이 `Open` 상태로 남거나 전체 세션 로그에 종료 기록이 누락되던 문제와, 종료 중 진행 중인 요청·터널이 간헐적으로 실패로 기록되던 문제를 수정한다.
