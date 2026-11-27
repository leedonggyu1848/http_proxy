//! Inspection terminal toolbar button UI text catalog.

define_catalog! {
    pub(crate) struct TerminalText {
        open_terminal,
    }
    tooltips: pub(crate) struct TerminalTooltips {
        opens_shell,
        start_proxy_first,
    }
    en = {
        open_terminal: "Open Terminal",
    }
    en_tooltips = {
        opens_shell: "Opens a shell whose programs send HTTP and HTTPS through this proxy.",
        start_proxy_first: "Start the proxy to open an inspection terminal.",
    }
    ko = {
        open_terminal: "터미널 열기",
    }
    ko_tooltips = {
        opens_shell: "실행한 프로그램의 HTTP와 HTTPS가 이 프록시를 거치는 셸을 엽니다.",
        start_proxy_first: "검사 터미널을 열려면 프록시를 시작하세요.",
    }
}
