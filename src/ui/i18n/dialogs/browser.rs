//! Browser launcher toolbar menu UI text catalog.

define_catalog! {
    pub(crate) struct BrowserText {
        close_browser,
        browser_open,
        opening_browser,
        open_browser_menu,
        no_browsers_found,
        refresh_browsers,
    }
    tooltips: pub(crate) struct BrowserTooltips {
        profile,
        chrome_shared_ca,
        firefox_profile_ca,
    }
    en = {
        close_browser: "Close browser",
        browser_open: "Browser open",
        opening_browser: "Opening browser…",
        open_browser_menu: "Open Browser ▼",
        no_browsers_found: "No Firefox or Chrome found on PATH.",
        refresh_browsers: "Refresh browsers",
    }
    en_tooltips = {
        profile: "Opens a separate private profile that closes when the proxy stops.",
        chrome_shared_ca: "Registers the CA in your user's shared Chrome/Chromium store until you remove it in CA details.",
        firefox_profile_ca: "Firefox trusts the CA only in the temporary inspection profile.",
    }
    ko = {
        close_browser: "브라우저 닫기",
        browser_open: "브라우저 열림",
        opening_browser: "브라우저 여는 중…",
        open_browser_menu: "브라우저 열기 ▼",
        no_browsers_found: "PATH에서 Firefox 또는 Chrome을 찾지 못했습니다.",
        refresh_browsers: "브라우저 목록 새로고침",
    }
    ko_tooltips = {
        profile: "프록시가 중지되면 함께 닫히는 별도의 임시 비공개 프로필로 엽니다.",
        chrome_shared_ca: "CA 정보에서 제거할 때까지 현재 사용자의 공용 Chrome/Chromium 저장소에 CA를 등록합니다.",
        firefox_profile_ca: "Firefox는 임시 검사 프로필 안에서만 이 CA를 신뢰합니다.",
    }
}
