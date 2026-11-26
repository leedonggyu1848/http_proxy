//! The command line grammar. Arguments are parsed here and converted into
//! protocol requests before anything else runs.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use http_inspector_analysis::filter::RecordFilter;
use http_inspector_analysis::search::{MatchOptions, SearchScope};
use http_inspector_exchange::BodySide;

use crate::protocol::{
    BodyRequest, BodyView, FilterSpec, Kind, MAX_WAIT, Part, RecordRef, Request, StartOptions,
    TreePath,
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
    Image,
}

#[derive(Debug, Args)]
pub(crate) struct WatchArgs {
    #[command(flatten)]
    pub(crate) filter: FilterArgs,
    /// Also report retained records with a larger ID. Defaults to new records only.
    #[arg(long)]
    pub(crate) since: Option<u64>,
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
                ViewArg::Image => BodyView::Image,
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
            },
            Self::Wait(args) => Request::Wait {
                filter: FilterSpec::from(&args.filter),
                since: args.since.map(RecordRef::new),
                timeout: args.timeout,
            },
            Self::Clear => Request::Clear,
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
    }
}
