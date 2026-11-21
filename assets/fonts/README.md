# 내장 폰트

`NotoSansKR-Regular-subset.otf`는 앱이 OS와 폰트 설치 여부와 관계없이 한글을 표시하도록 바이너리에 `include_bytes!`로 넣는 폰트다. `src/ui/fonts.rs`가 egui 기본 폰트 뒤, 시스템 CJK 폰트 앞에 등록한다.

## 원본

| 항목 | 값 |
|---|---|
| 폰트 | Noto Sans KR Regular, 버전 2.004 |
| 배포처 | [notofonts/noto-cjk 릴리스 `Sans2.004`](https://github.com/notofonts/noto-cjk/releases/tag/Sans2.004)의 `17_NotoSansKR.zip` |
| `17_NotoSansKR.zip` SHA-256 | `ac7eeb4e2b0d41de8ff31b2d6e1e2a41caf253fd5cefb380bfa1f40f1747b612` |
| `NotoSansKR-Regular.otf` SHA-256 | `69975a0ac8472717870aefeab0a4d52739308d90856b9955313b2ad5e0148d68` (4,644,748 bytes) |
| 라이선스 | SIL Open Font License 1.1, [`OFL.txt`](OFL.txt) |

`OFL.txt`는 저작권 줄이 포함된 Google Fonts 배포본(`google/fonts`의 `ofl/notosanskr/OFL.txt`, 커밋 `06bfe0d62a98375cb09c942fa18e68bd32cff057`)을 그대로 복사했다. 릴리스 zip의 `LICENSE`에는 저작권·Reserved Font Name 줄이 없어 이 파일을 쓴다. 라이선스 조항은 두 파일이 같다.

## 서브셋 범위

| 범위 | 내용 |
|---|---|
| `U+0020-007E` | ASCII |
| `U+00A0-00FF` | Latin-1 문장 부호와 기호 |
| `U+2010-2027`, `U+2030-205E` | 일반 문장 부호(대시, 따옴표, 말줄임표 등) |
| `U+20A9`, `U+20AC` | ₩, € |
| `U+2190-2193` | 화살표 |
| `U+3000-303F` | CJK 기호와 문장 부호(「」, 『』, 〈〉 등) |
| `U+3130-318F` | 한글 호환 자모 |
| `U+AC00-D7A3` | 현대 한글 음절 11,172자 |
| `U+FF01-FF5E` | 전각 ASCII |

한자와 가나는 용량 때문에 넣지 않고 시스템 CJK 폰트에 맡긴다. 한글 음절은 미리 조합된 글리프라 셰이핑이 필요 없으므로 `GSUB`·`GPOS` 기능과 힌팅을 빼고, egui가 쓰지 않는 세로쓰기 표(`vhea`, `vmtx`, `VORG`)를 지운다.

## 재생성

fontTools 4.55.1의 `pyftsubset`으로 만들었다. 같은 버전에서 같은 명령을 다시 실행하면 바이트 단위로 같은 파일이 나온다.

```bash
gh release download Sans2.004 --repo notofonts/noto-cjk --pattern 17_NotoSansKR.zip
shasum -a 256 17_NotoSansKR.zip
unzip 17_NotoSansKR.zip NotoSansKR-Regular.otf
pyftsubset NotoSansKR-Regular.otf \
	--unicodes="U+0020-007E,U+00A0-00FF,U+2010-2027,U+2030-205E,U+20A9,U+20AC,U+2190-2193,U+3000-303F,U+3130-318F,U+AC00-D7A3,U+FF01-FF5E" \
	--layout-features='' \
	--no-hinting \
	--notdef-outline \
	--drop-tables+=vhea,vmtx,VORG \
	--output-file=NotoSansKR-Regular-subset.otf
```

결과 파일은 1,148,724 bytes, SHA-256은 `0b9ae4d6523e4195879830974247179d2339c3fe7633cffed25a9897a58a1ab5`이다.

## 폰트 이름

OFL 3항은 수정본이 저작권자가 지정한 Reserved Font Name을 쓰지 못하게 한다. Noto Sans KR의 Reserved Font Name은 `Source`뿐이므로 서브셋도 `Noto Sans KR` 이름을 유지한다. 비교한 나눔고딕은 `Nanum`, `NanumGothic` 등을 Reserved Font Name으로 지정해 서브셋하면 이름을 바꿔야 한다.

앱 코드의 MIT License는 이 폰트에 적용되지 않는다. 폰트는 OFL 1.1에 따라 앱과 함께 배포하며 단독으로 판매하지 않는다.
