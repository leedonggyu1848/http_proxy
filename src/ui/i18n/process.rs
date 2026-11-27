//! Process attribution tab and process filter text.

use super::Language;
use http_inspector_exchange::ProcessUnavailable;

define_catalog! {
    pub(crate) struct ProcessText {
        tab,
        program,
        executable,
        pid,
        started,
        user,
        command,
        parents,
        browser,
        session,
        inspection_browser,
        state,
        exited,
        unknown,
        client_port,
        connection_id,
        requests,
        filter,
        clear_filter,
        pending,
        replay,
        unavailable,
        unsupported,
        socket_unavailable,
        owner_unavailable,
        permission_denied,
        busy,
        interrupted,
    }
    en = {
        tab: "Process",
        program: "Program",
        executable: "Executable",
        pid: "PID",
        started: "Started",
        user: "User",
        command: "Command line",
        parents: "Parent processes",
        browser: "Browser",
        session: "Session",
        inspection_browser: "Opened by HTTP Inspector",
        state: "State",
        exited: "Exited",
        unknown: "Unknown",
        client_port: "Client port",
        connection_id: "Connection ID",
        requests: "Connection requests",
        filter: "Filter by process",
        clear_filter: "Clear process filter",
        pending: "Looking up the client process…",
        replay: "Replayed in HTTP Inspector.",
        unavailable: "No client connection was recorded.",
        unsupported: "Process lookup is available on Linux.",
        socket_unavailable: "The client socket closed or is in another network namespace.",
        owner_unavailable: "The client process exited or its file descriptors are not visible.",
        permission_denied: "Linux denied access to the client process or socket.",
        busy: "The process lookup queue was full.",
        interrupted: "The proxy stopped during process lookup.",
    }
    ko = {
        tab: "프로세스",
        program: "프로그램",
        executable: "실행 파일",
        pid: "PID",
        started: "시작 시각",
        user: "사용자",
        command: "명령줄",
        parents: "부모 프로세스",
        browser: "브라우저",
        session: "세션",
        inspection_browser: "HTTP Inspector가 연 검사 브라우저",
        state: "상태",
        exited: "종료됨",
        unknown: "알 수 없음",
        client_port: "클라이언트 포트",
        connection_id: "연결 ID",
        requests: "연결 요청 수",
        filter: "프로세스로 필터",
        clear_filter: "프로세스 필터 해제",
        pending: "클라이언트 프로세스 조회 중…",
        replay: "HTTP Inspector에서 재전송했습니다.",
        unavailable: "기록된 클라이언트 연결이 없습니다.",
        unsupported: "Linux에서 프로세스를 조회할 수 있습니다.",
        socket_unavailable: "클라이언트 소켓이 닫혔거나 다른 네트워크 네임스페이스에 있습니다.",
        owner_unavailable: "클라이언트 프로세스가 종료됐거나 파일 디스크립터를 읽을 수 없습니다.",
        permission_denied: "클라이언트 프로세스나 소켓을 읽을 권한이 없습니다.",
        busy: "프로세스 조회 대기열이 가득 찼습니다.",
        interrupted: "프로세스를 조회하던 중 프록시가 중지됐습니다.",
    }
}

pub(crate) fn unavailable(lang: Language, reason: ProcessUnavailable) -> String {
    let text = lang.process();
    match reason {
        ProcessUnavailable::Unsupported => text.unsupported,
        ProcessUnavailable::SocketUnavailable => text.socket_unavailable,
        ProcessUnavailable::OwnerUnavailable => text.owner_unavailable,
        ProcessUnavailable::PermissionDenied => text.permission_denied,
        ProcessUnavailable::Busy => text.busy,
        ProcessUnavailable::Interrupted => text.interrupted,
        ProcessUnavailable::DifferentUser(uid) => {
            return match lang {
                Language::English => format!("The client belongs to a different user (UID {uid})."),
                Language::Korean => format!("다른 사용자의 클라이언트입니다(UID {uid})."),
            };
        }
    }
    .to_owned()
}
