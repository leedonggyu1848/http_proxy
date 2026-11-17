# HTTPS 복호화 구현 설계

## 문제

현재 프록시는 CONNECT 트래픽을 읽지 않고 전달한다. HTTPS 복호화는 기본 터널 동작을 바꾸지 않으면서 양쪽 TLS를 종료해야 한다. 기존 스트리밍 캡처를 유지하고 실제 서버 인증서를 검증해야 한다. CONNECT 대상은 연결이 끝날 때까지 바뀌면 안 된다.

## 사용자 흐름

앱은 터널 모드로 시작한다. 사용자는 프록시를 중지하고 HTTPS 복호화를 활성화한 다음 다시 시작한다. 앱은 이 설치에서만 사용하는 CA를 생성하거나 불러온다. UI는 공개 인증서 경로와 SHA-256 지문을 표시한다. 사용자는 공개 인증서만 클라이언트 신뢰 저장소에 등록한다.

앱은 브라우저나 운영체제의 신뢰 저장소를 수정하지 않는다. 실행 중인 프록시의 모드는 시작할 때 고정한다.

## 공개 타입

```rust
pub struct HttpsInspection {
	ca_directory: PathBuf,
	additional_upstream_root: Option<PathBuf>,
}

impl HttpsInspection {
	pub fn new(ca_directory: PathBuf) -> Self;
	pub fn with_additional_upstream_root(self, path: PathBuf) -> Self;
}

pub struct ProxyConfig {
	// 기존 필드
	https_inspection: Option<HttpsInspection>,
}

impl ProxyConfig {
	pub fn with_https_inspection(self, config: HttpsInspection) -> Self;
}

pub struct CaInfo {
	certificate_path: PathBuf,
	fingerprint_sha256: String,
}

impl ProxyHandle {
	pub fn ca_info(&self) -> Option<&CaInfo>;
}
```

기본 설정에는 `HttpsInspection`이 없으며 기존 CONNECT 터널을 유지한다. TLS 라이브러리 타입은 공개 API에 노출하지 않는다.

## CA 소유권

비공개 TLS 모듈이 CA 저장, 인증서 발급, 업스트림 루트 인증서와 rustls 설정을 소유한다. 복호화 모드는 권한이 `0700`인 디렉터리를 만든다. 모듈은 인증서와 개인키를 권한이 `0600`인 단일 원본 파일에 저장한다. 가져오기용 공개 인증서는 별도 경로에 저장한다.

모듈은 CA를 생성하거나 읽기 전에 프로세스 잠금을 얻는다. 임시 파일을 완전히 기록하고 디스크에 반영한 다음 원본 파일 이름으로 바꾼다. 기존 개인키 파일은 일반 파일이어야 하며 안전한 권한을 가져야 한다. 파일이 없거나 손상되었거나 인증서와 키가 맞지 않으면 시작을 중단한다. 이미 신뢰한 CA를 자동으로 교체하지 않는다.

루트 인증서는 ECDSA P-256 키, 제한된 CA 기본 제약과 인증서 서명 용도를 사용한다. 각 CONNECT는 DNS 이름 또는 IP 주소가 SAN에 들어간 단기 leaf 인증서를 발급받는다. 첫 구현은 leaf 인증서를 공유 캐시에 저장하지 않는다.

## CONNECT 처리

터널 모드는 기존 TCP 복사 경로를 사용한다. 복호화 모드는 다음 순서로 처리한다.

1. CONNECT authority를 파싱하고 검증한다.
2. 상태 200을 보내기 전에 대상 인증서를 준비한다.
3. 업그레이드 작업을 생성하고 상태 200을 반환한다.
4. HTTP/1.1만 ALPN으로 제공하며 클라이언트 TLS를 수락한다.
5. 복호화된 스트림에서 Hyper HTTP/1.1 서버를 실행한다.
6. 모든 내부 요청의 목적지를 CONNECT 호스트와 포트로 고정한다.
7. 인증서와 호스트 이름을 검증하며 실제 서버에 TLS로 연결한다.
8. 기존 제한형 캡처 경로로 origin-form 요청을 전달한다.

내부 요청은 다른 `Host`나 absolute URI로 목적지를 바꿀 수 없다. 캡처에는 `https://` 절대 URL을 기록한다. 실제 서버에는 origin-form 요청을 보낸다.

CONNECT 준비 실패는 상태 200 전에 HTTP 오류로 반환한다. 업그레이드와 클라이언트 TLS 실패는 상태 200 이후에 발생하므로 연결을 닫고 실패 기록을 게시한다. 업스트림 TLS 실패는 복호화된 HTTP 안에서 발생하므로 암호화된 상태 502 응답과 내부 요청 실패 기록을 반환한다.

## 실행 범위

TLS handshake와 연결 작업은 프록시 런타임에서 실행한다. handshake에는 timeout을 적용한다. 프록시를 중지하면 현재 동작과 같이 런타임이 활성 연결 작업을 종료한다. 본문 관찰기가 중간에 제거되면 미리보기 상태는 중단으로 바뀐다.

첫 구현은 HTTP/1.1만 지원한다. HTTP/2를 광고하지 않는다. 인증서 검증을 끄지 않으며 인증서 고정이나 클라이언트 인증서를 우회하지 않는다. 앱은 신뢰 저장소를 수정하지 않는다.

## 설계 선택

하나의 비공개 TLS 모듈이 보안 정책을 소유한다. 한 번 시작한 프록시는 변경되지 않는 모드를 사용한다. 평문 HTTP와 복호화된 HTTPS는 같은 캡처 경로를 사용한다. CONNECT마다 leaf 인증서를 발급해 첫 버전에서 공유 캐시 상태를 제거한다.

실행 중 모드를 바꾸는 설계는 UI와 네트워크 작업이 같은 잠금을 사용해야 해서 제외했다. 이 설계는 터널만 사용하는 사용자에게도 CA 파일을 만들었다. 별도 HTTPS 전달 경로는 헤더 제거, 본문 캡처와 오류 처리를 중복하므로 제외했다.

## 검증 계약

테스트는 다음 동작을 증명해야 한다.

- 기본 CONNECT가 임의 바이트를 계속 전달한다.
- 생성된 CA를 신뢰한 클라이언트가 프록시를 통해 HTTPS를 전송한다.
- 캡처 대상에 CONNECT authority와 HTTPS scheme이 들어간다.
- 요청과 응답 헤더, 제한된 본문이 기존 마스킹 규칙을 사용한다.
- 내부 authority가 다르면 요청을 거부한다.
- 신뢰할 수 없거나 호스트 이름이 다른 업스트림 인증서를 거부한다.
- 재시작 후에도 CA 지문과 개인키 권한을 유지한다.
- 클라이언트 TLS 실패 시 평문 HTTP 응답을 추가로 보내지 않고 연결을 닫는다.
- TLS handshake 중 프록시를 중지해도 종료가 멈추지 않는다.

## 구현 조정

비공개 함수 이름과 스트림 타입은 구현 과정에서 바꿀 수 있다. 공개 설정, CA 저장 규칙, 목적지 고정, 인증서 검증 또는 실패 동작을 바꾸면 같은 커밋에서 이 문서를 갱신한다.
