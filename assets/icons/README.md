# 앱 아이콘

`http-inspector.svg`는 이 저장소에서 직접 그린 앱 아이콘이며 코드와 같은 [MIT License](../../LICENSE)를 따른다. 요청·응답을 뜻하는 두 화살표와 검사를 뜻하는 돋보기로 구성한다.

Linux `.deb` 패키지는 SVG를 `/usr/share/icons/hicolor/scalable/apps/`에, PNG를 `/usr/share/icons/hicolor/256x256/apps/`에 설치한다. 데스크톱 항목(`packaging/linux/http-inspector.desktop`)은 `Icon=http-inspector`로 이 아이콘을 찾는다.

SVG를 고치면 [resvg](https://github.com/linebender/resvg) CLI(`cargo install --locked resvg`)로 PNG를 다시 만든다.

```bash
resvg -w 256 -h 256 assets/icons/http-inspector.svg assets/icons/http-inspector-256.png
```
