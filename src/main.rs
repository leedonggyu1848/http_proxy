#![forbid(unsafe_code)]

pub mod ui;

use std::process::ExitCode;

/// Runs a command when the first argument names one, such as
/// `http-proxy proxy start`; otherwise opens the desktop app.
fn main() -> Result<ExitCode, eframe::Error> {
    let args: Vec<_> = std::env::args_os().collect();
    if http_inspector_cli::is_cli_invocation(&args) {
        return Ok(http_inspector_cli::run(args));
    }
    ui::main().map(|()| ExitCode::SUCCESS)
}
