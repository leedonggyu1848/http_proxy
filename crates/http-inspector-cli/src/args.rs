//! The command line grammar. Arguments are parsed here and converted into
//! protocol requests before anything else runs.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use http_inspector_analysis::filter::RecordFilter;
use http_inspector_analysis::search::{MatchOptions, SearchScope};
use http_inspector_exchange::BodySide;

use crate::protocol::{
    BodyRequest, BodyView, ExportRequest, FilterSpec, HlsRequest, Kind, MAX_WAIT, Part, RecordRef,
    ReplayRequest, Request, StartOptions, TreePath,
};

const EXAMPLES: &str = "\
Examples:
  http-proxy proxy start --port 0 --json
  curl -x http://127.0.0.1:8080 http://example.test/api/users
  http-proxy records wait --filter 'method:GET url:/api/users' --timeout 10s --json
  http-proxy records body 42 --side response --view pretty --max-bytes 4096
  http-proxy log save ./session.log
  http-proxy proxy stop

Without a subcommand, http-proxy opens the desktop app.
Commands, JSON output, and exit codes: `http-proxy schema` or docs/cli.md.";

/// HTTP Inspector command line. Every command accepts `--json`.
#[derive(Debug, Parser)]
#[command(
    name = "http-proxy",
    version,
    about = "Inspect HTTP traffic through a local proxy from the command line",
    after_help = EXAMPLES
)]
pub(crate) struct Cli {
    /// Print one JSON document (NDJSON for streams) with `"schema": 1`.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Start, stop, and inspect the proxy daemon.
    #[command(subcommand)]
    Proxy(ProxyCommand),
    /// List, read, watch, and clear captured records.
    #[command(subcommand)]
    Records(RecordsCommand),
    /// Save the full session log.
    #[command(subcommand)]
    Log(LogCommand),
    /// Show the local HTTPS inspection CA certificate.
    #[command(subcommand)]
    Ca(CaCommand),
    /// Open an interactive shell whose programs use the running proxy.
    /// Its CA bundle is removed when the shell exits.
    #[command(
        after_help = "Example:\n  http-proxy shell\n  curl https://example.test/   # captured, no options needed\n  exit"
    )]
    Shell(ShellArgs),
    /// Run one command through the running proxy and return its exit code.
    #[command(
        after_help = "Example:\n  http-proxy exec -- curl -sS https://example.test/api/users"
    )]
    Exec(ExecArgs),
    /// Print every command, output field, error code, and exit code as JSON.
    Schema,
    /// Run the daemon in this process (used by `proxy start`).
    #[command(name = "__daemon", hide = true)]
    Daemon(StartArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProxyCommand {
    /// Start the daemon and proxy, or report the running one.
    #[command(after_help = "Example:\n  http-proxy proxy start --port 0 --https-inspection --json")]
    Start(StartCommand),
    /// Stop the proxy and the daemon. Unsaved records are discarded.
    Stop,
    /// Show the proxy address, HTTPS inspection, and record counts.
    Status,
    /// Change a setting of the running proxy.
    #[command(after_help = "Example:\n  http-proxy proxy set --https-inspection on")]
    Set(SetArgs),
    /// Print shell `export` lines that send programs through the proxy.
    #[command(after_help = "Example:\n  eval \"$(http-proxy proxy env)\"")]
    Env(EnvArgs),
}

#[derive(Debug, Args)]
pub(crate) struct EnvArgs {
    /// Also write a bundle of the system roots and the local CA and export
    /// `SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `REQUESTS_CA_BUNDLE`,
    /// `GIT_SSL_CAINFO`, `AWS_CA_BUNDLE`, and `DENO_CERT` naming it.
    /// `proxy stop` removes the bundle.
    #[arg(long)]
    pub(crate) ca_bundle: bool,
}

/// Options shared by `shell` and `exec`.
#[derive(Debug, Args)]
pub(crate) struct InspectionArgs {
    /// Keep inherited `NO_PROXY` and `no_proxy`. By default they are removed
    /// so requests to local servers are captured too.
    #[arg(long)]
    pub(crate) keep_no_proxy: bool,
    /// Use this proxy instead of asking the daemon (used by the app).
    #[arg(long, hide = true)]
    pub(crate) proxy: Option<std::net::SocketAddr>,
    /// Trust the CA in this directory (used by the app with `--proxy`).
    #[arg(long, hide = true, requires = "proxy")]
    pub(crate) ca_directory: Option<PathBuf>,
    /// Wait for Enter after a setup error, so a new terminal window does
    /// not close before the error is read (used by the app).
    #[arg(long, hide = true)]
    pub(crate) hold_on_error: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ShellArgs {
    #[command(flatten)]
    pub(crate) options: InspectionArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ExecArgs {
    #[command(flatten)]
    pub(crate) options: InspectionArgs,
    /// The command and its arguments, after `--`.
    #[arg(
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "COMMAND",
        value_parser = clap::value_parser!(std::ffi::OsString)
    )]
    pub(crate) command: Vec<std::ffi::OsString>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CaCommand {
    /// Print the CA certificate path, SHA-256 fingerprint, and validity.
    /// Creates the CA when it does not exist. The private key is never printed.
    Info,
}

#[derive(Debug, Args)]
pub(crate) struct StartArgs {
    /// Loopback port; 0 picks a free port. Defaults to the saved port or 8080.
    #[arg(long)]
    pub(crate) port: Option<u16>,
    /// Decrypt HTTPS with the local CA (off by default, as in the app).
    #[arg(long)]
    pub(crate) https_inspection: bool,
}

#[derive(Debug, Args)]
pub(crate) struct StartCommand {
    #[command(flatten)]
    pub(crate) options: StartArgs,
    /// Serve in this process instead of a background daemon.
    #[arg(long)]
    pub(crate) foreground: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SetArgs {
    /// Restart the proxy on the same port with HTTPS inspection on or off.
    #[arg(long, value_enum)]
    pub(crate) https_inspection: OnOff,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum OnOff {
    On,
    Off,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RecordsCommand {
    /// List retained records, oldest first.
    #[command(after_help = "Example:\n  http-proxy records list --filter 'status:4xx' --limit 20")]
    List(ListArgs),
    /// Show headers, query parameters, timing, and the outcome of a record.
    Show(ShowArgs),
    /// Print a body as Pretty, Raw, Tree, Hex, or image information.
    #[command(
        after_help = "Examples:\n  http-proxy records body 42 --view pretty --max-bytes 4096\n  http-proxy records body 42 --json-path '$.data.items[3].id'\n  http-proxy records body 42 --xpath '/rss/channel/item[2]/title'"
    )]
    Body(BodyArgs),
    /// Stream new and changed records as NDJSON until interrupted.
    Watch(WatchArgs),
    /// Wait until a completed record matches, print it, and exit.
    #[command(
        after_help = "Example:\n  http-proxy records wait --filter 'method:POST url:/login' --timeout 10s"
    )]
    Wait(WaitArgs),
    /// Remove every retained record, as the app's Clear does.
    Clear,
    /// Send a captured request again, optionally changed, and print the new record.
    #[command(
        after_help = "Examples:\n  http-proxy records replay 42 --header 'X-Debug: 1'\n  http-proxy records replay 42 --method POST --body-file body.json --yes\n  http-proxy records replay 42 --dry-run --json"
    )]
    Replay(ReplayArgs),
    /// Print a captured request as a `curl` command.
    Curl(CurlArgs),
    /// Write matching records as a HAR 1.2 file, or print it.
    #[command(
        after_help = "Examples:\n  http-proxy records export --output session.har\n  http-proxy records export --filter 'host:api.example.test' --include-bodies > api.har"
    )]
    Export(ExportArgs),
    /// Show the request and response structure of each endpoint, or an `OpenAPI` document.
    #[command(
        after_help = "Examples:\n  http-proxy records endpoints --filter 'host:api.example.test' --json\n  http-proxy records endpoints --filter 'host:api.example.test' --format openapi > api.json"
    )]
    Endpoints(EndpointsArgs),
    /// Inspect a captured HLS playlist, or download a variant as one `.ts` file.
    #[command(
        after_help = "Examples:\n  http-proxy records hls 12\n  http-proxy records hls 12 --variant 1 --output video.ts"
    )]
    Hls(HlsArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    #[command(flatten)]
    pub(crate) filter: FilterArgs,
    /// File format. HAR 1.2 is the only one.
    #[arg(long, value_enum, default_value = "har")]
    pub(crate) format: ExportFormat,
    /// Write to this new file; an existing file is never overwritten. Without it, print the HAR.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Include body previews, which may be truncated, as in the app's export.
    #[arg(long)]
    pub(crate) include_bodies: bool,
    /// Include credential headers with their `[REDACTED]` marker instead of leaving them out.
    #[arg(long)]
    pub(crate) include_redacted_headers: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ExportFormat {
    Har,
}

#[derive(Debug, Args)]
pub(crate) struct EndpointsArgs {
    #[command(flatten)]
    pub(crate) filter: FilterArgs,
    /// `summary` lists observed structure; `openapi` prints an `OpenAPI` 3.1 document for one host.
    #[arg(long, value_enum, default_value = "summary")]
    pub(crate) format: EndpointsFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum EndpointsFormat {
    Summary,
    Openapi,
}

#[derive(Debug, Args)]
pub(crate) struct HlsArgs {
    /// ID of a captured `.m3u8` playlist record.
    pub(crate) id: u64,
    /// Variant index from the inspection; defaults to the app's choice.
    #[arg(long)]
    pub(crate) variant: Option<usize>,
    /// Download the variant into this new `.ts` file; an existing file is never overwritten.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ReplayArgs {
    /// Record ID from `records list`.
    pub(crate) id: u64,
    /// Replace the method.
    #[arg(long)]
    pub(crate) method: Option<String>,
    /// Replace the absolute http:// or https:// URL.
    #[arg(long)]
    pub(crate) url: Option<String>,
    /// Set a header, replacing captured headers of the same name: `'Name: value'`.
    #[arg(long = "header", value_parser = parse_header)]
    pub(crate) headers: Vec<(String, String)>,
    /// Remove every captured header with this name.
    #[arg(long = "remove-header")]
    pub(crate) remove_headers: Vec<String>,
    /// Send this file as the complete body (at most 16 MiB).
    #[arg(long)]
    pub(crate) body_file: Option<PathBuf>,
    /// Confirm a request that may change server state (methods other than GET, HEAD, OPTIONS).
    #[arg(long)]
    pub(crate) yes: bool,
    /// Print the request that would be sent, without sending it.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// How long to wait for the replayed exchange to finish.
    #[arg(long, default_value = "60s", value_parser = parse_duration)]
    pub(crate) timeout: Duration,
}

#[derive(Debug, Args)]
pub(crate) struct CurlArgs {
    /// Record ID from `records list`.
    pub(crate) id: u64,
    /// Keep redacted headers as `<REDACTED>` placeholders instead of omitting them.
    #[arg(long)]
    pub(crate) placeholders: bool,
}

#[derive(Debug, Args)]
pub(crate) struct FilterArgs {
    /// Filter text in the app's syntax, such as `method:GET status:4xx users`.
    #[arg(long, default_value = "")]
    pub(crate) filter: String,
    /// Where free text is searched.
    #[arg(long, value_enum, default_value = "all")]
    pub(crate) scope: ScopeArg,
    /// Match letter case exactly.
    #[arg(long)]
    pub(crate) case_sensitive: bool,
    /// Read the whole filter as one regular expression.
    #[arg(long)]
    pub(crate) regex: bool,
    /// Outcome kind, as the app's All, Responses, Failures, and Tunnels tabs.
    #[arg(long, value_enum, default_value = "all")]
    pub(crate) kind: KindArg,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ScopeArg {
    All,
    Url,
    Headers,
    RequestBody,
    ResponseBody,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum KindArg {
    All,
    Responses,
    Failures,
    Tunnels,
}

#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    #[command(flatten)]
    pub(crate) filter: FilterArgs,
    /// Only records with a larger ID.
    #[arg(long)]
    pub(crate) since: Option<u64>,
    /// Only the newest N matching records.
    #[arg(long)]
    pub(crate) limit: Option<usize>,
    /// Print every record field instead of the summary.
    #[arg(long)]
    pub(crate) full: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// Record ID from `records list`.
    pub(crate) id: u64,
    /// Show only the request or the response.
    #[arg(long, value_enum, default_value = "all")]
    pub(crate) part: PartArg,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum PartArg {
    All,
    Request,
    Response,
}

#[derive(Debug, Args)]
pub(crate) struct BodyArgs {
    /// Record ID from `records list`.
    pub(crate) id: u64,
    /// Which body to print.
    #[arg(long, value_enum, default_value = "response")]
    pub(crate) side: SideArg,
    /// View mode. Defaults to `image` for images and `pretty` otherwise.
    #[arg(long, value_enum)]
    pub(crate) view: Option<ViewArg>,
    /// Cut the printed text to N bytes; JSON reports `truncated`.
    #[arg(long)]
    pub(crate) max_bytes: Option<usize>,
    /// Print the JSON Tree row with this path, such as `$.items[0].id`.
    #[arg(long, conflicts_with_all = ["xpath", "view"])]
    pub(crate) json_path: Option<String>,
    /// Print the XML Tree row with this `XPath`, such as `/rss/channel/title`.
    #[arg(long, conflicts_with = "view")]
    pub(crate) xpath: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum SideArg {
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ViewArg {
    Pretty,
    Raw,
    Tree,
    Hex,
}

#[derive(Debug, Args)]
pub(crate) struct WatchArgs {
    #[command(flatten)]
    pub(crate) filter: FilterArgs,
    /// Also report retained records with a larger ID. Defaults to new records only.
    #[arg(long)]
    pub(crate) since: Option<u64>,
    /// Print every record field instead of the summary.
    #[arg(long)]
    pub(crate) full: bool,
}

#[derive(Debug, Args)]
pub(crate) struct WaitArgs {
    #[command(flatten)]
    pub(crate) filter: FilterArgs,
    /// Only records with a larger ID. Defaults to every retained record.
    #[arg(long)]
    pub(crate) since: Option<u64>,
    /// How long to wait, such as `500ms`, `10s`, or `2m` (at most 1h).
    #[arg(long, default_value = "30s", value_parser = parse_duration)]
    pub(crate) timeout: Duration,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LogCommand {
    /// Write every exchange of this daemon session to a new file.
    Save {
        /// Destination; an existing file is never overwritten.
        path: PathBuf,
    },
}

/// Parses `Name: value`. The name and value are checked when the request
/// is built, by the same rules as the app's editor.
pub(crate) fn parse_header(text: &str) -> Result<(String, String), String> {
    let (name, value) = text
        .split_once(':')
        .ok_or_else(|| format!("{text:?} is not a header such as 'X-Debug: 1'"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("{text:?} has no header name"));
    }
    Ok((name.to_owned(), value.trim_start().to_owned()))
}

/// Parses `500ms`, `10s`, `2m`, `1h`, or a bare number of seconds.
pub(crate) fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: u64 = number
        .parse()
        .map_err(|_| format!("{text:?} is not a duration such as 10s"))?;
    let duration = match unit {
        "ms" => Duration::from_millis(value),
        "" | "s" => Duration::from_secs(value),
        "m" => Duration::from_secs(value.saturating_mul(60)),
        "h" => Duration::from_secs(value.saturating_mul(3600)),
        _ => return Err(format!("unknown unit {unit:?}; use ms, s, m, or h")),
    };
    if duration > MAX_WAIT {
        return Err("the timeout must be at most 1h".to_owned());
    }
    Ok(duration)
}

impl From<&StartArgs> for StartOptions {
    fn from(args: &StartArgs) -> Self {
        Self {
            port: args.port,
            https_inspection: args.https_inspection,
        }
    }
}

impl From<&FilterArgs> for FilterSpec {
    fn from(args: &FilterArgs) -> Self {
        Self {
            text: args.filter.clone(),
            scope: match args.scope {
                ScopeArg::All => SearchScope::All,
                ScopeArg::Url => SearchScope::Url,
                ScopeArg::Headers => SearchScope::Headers,
                ScopeArg::RequestBody => SearchScope::RequestBody,
                ScopeArg::ResponseBody => SearchScope::ResponseBody,
            },
            options: MatchOptions {
                case_sensitive: args.case_sensitive,
                regex: args.regex,
            },
            kind: Kind(match args.kind {
                KindArg::All => RecordFilter::All,
                KindArg::Responses => RecordFilter::Responses,
                KindArg::Failures => RecordFilter::Failures,
                KindArg::Tunnels => RecordFilter::Tunnels,
            }),
        }
    }
}

impl From<&BodyArgs> for BodyRequest {
    fn from(args: &BodyArgs) -> Self {
        Self {
            id: RecordRef::new(args.id),
            side: match args.side {
                SideArg::Request => BodySide::Request,
                SideArg::Response => BodySide::Response,
            },
            view: args.view.map(|view| match view {
                ViewArg::Pretty => BodyView::Pretty,
                ViewArg::Raw => BodyView::Raw,
                ViewArg::Tree => BodyView::Tree,
                ViewArg::Hex => BodyView::Hex,
            }),
            max_bytes: args.max_bytes,
            path: args
                .json_path
                .clone()
                .map(TreePath::Json)
                .or_else(|| args.xpath.clone().map(TreePath::Xml)),
        }
    }
}

impl RecordsCommand {
    /// The daemon request for this command.
    pub(crate) fn request(&self) -> Request {
        match self {
            Self::List(args) => Request::List {
                filter: FilterSpec::from(&args.filter),
                since: args.since.map(RecordRef::new),
                limit: args.limit,
                full: args.full,
            },
            Self::Show(args) => Request::Show {
                id: RecordRef::new(args.id),
                part: match args.part {
                    PartArg::All => Part::All,
                    PartArg::Request => Part::Request,
                    PartArg::Response => Part::Response,
                },
            },
            Self::Body(args) => Request::Body(BodyRequest::from(args)),
            Self::Watch(args) => Request::Watch {
                filter: FilterSpec::from(&args.filter),
                since: args.since.map(RecordRef::new),
                full: args.full,
            },
            Self::Wait(args) => Request::Wait {
                filter: FilterSpec::from(&args.filter),
                since: args.since.map(RecordRef::new),
                timeout: args.timeout,
            },
            Self::Clear => Request::Clear,
            Self::Replay(args) => Request::Replay(ReplayRequest {
                id: RecordRef::new(args.id),
                method: args.method.clone(),
                url: args.url.clone(),
                headers: args.headers.clone(),
                remove_headers: args.remove_headers.clone(),
                body_file: args.body_file.clone(),
                yes: args.yes,
                dry_run: args.dry_run,
                timeout: args.timeout,
            }),
            Self::Curl(args) => Request::Curl {
                id: RecordRef::new(args.id),
                placeholders: args.placeholders,
            },
            Self::Export(args) => Request::Export(ExportRequest {
                filter: FilterSpec::from(&args.filter),
                output: args.output.clone(),
                include_bodies: args.include_bodies,
                include_redacted_headers: args.include_redacted_headers,
            }),
            Self::Endpoints(args) => Request::Endpoints {
                filter: FilterSpec::from(&args.filter),
                openapi: matches!(args.format, EndpointsFormat::Openapi),
            },
            Self::Hls(args) => Request::Hls(HlsRequest {
                id: RecordRef::new(args.id),
                variant: args.variant,
                output: args.output.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn the_grammar_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("10"), Ok(Duration::from_secs(10)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert!(parse_duration("2d").is_err());
        assert!(parse_duration("2h").is_err());
        assert!(parse_duration("s").is_err());
        assert_eq!(
            parse_header("X-Debug:  1 2"),
            Ok(("X-Debug".to_owned(), "1 2".to_owned()))
        );
        assert!(parse_header("no colon").is_err());
        assert!(parse_header(": value").is_err());
    }
}
