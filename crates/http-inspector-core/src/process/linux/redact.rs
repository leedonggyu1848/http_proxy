//! Command arguments are sanitized before they leave the process lookup.

use std::fmt::Write as _;
use std::net::Ipv6Addr;
use std::path::Path;

use http_inspector_exchange::http_semantics::{REDACTED_VALUE, is_sensitive_header};

#[derive(Clone, Copy)]
enum Value {
    Secret,
    Header { expanded: bool },
    Authority { curl: bool },
    Variable,
}

impl Value {
    fn redact(self, argument: &str) -> String {
        match self {
            Self::Secret => REDACTED_VALUE.to_owned(),
            Self::Header { expanded } => redact_header(argument, expanded),
            Self::Authority { curl } => redact_authority(argument, curl),
            Self::Variable => redact_variable(argument),
        }
    }
}

struct OptionValue<'a> {
    name: &'a str,
    inline: Option<(&'static str, &'a str)>,
    value: Value,
}

pub(super) fn command(bytes: &[u8], executable: Option<&Path>) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let mut arguments = bytes
        .strip_suffix(&[0])
        .unwrap_or(bytes)
        .split(|&byte| byte == 0)
        .map(String::from_utf8_lossy);
    let program = arguments.next()?;
    let curl = [executable, Some(Path::new(program.as_ref()))]
        .into_iter()
        .flatten()
        .any(|path| path.file_name().is_some_and(|name| name == "curl"));

    let mut result = String::new();
    append_argument(&mut result, &redact_urls(&program));
    let mut next = None::<Value>;
    for argument in arguments {
        result.push(' ');
        if let Some(value) = next.take() {
            append_argument(&mut result, &value.redact(&argument));
            continue;
        }
        let redacted = option_value(&argument, curl).map_or_else(
            || {
                if curl && !argument.starts_with('-') {
                    redact_authority(&argument, true)
                } else {
                    redact_urls(&argument)
                }
            },
            |option| {
                if let Some((separator, value)) = option.inline {
                    format!("{}{separator}{}", option.name, option.value.redact(value))
                } else {
                    next = Some(option.value);
                    option.name.to_owned()
                }
            },
        );
        append_argument(&mut result, &redacted);
    }
    Some(result)
}

fn option_value(argument: &str, curl: bool) -> Option<OptionValue<'_>> {
    if curl && argument.starts_with('-') && !argument.starts_with("--") {
        return curl_short_option(argument);
    }
    let (name, inline) = argument
        .split_once('=')
        .map_or((argument, None), |(name, value)| (name, Some(("=", value))));
    let expansion = if curl {
        name.strip_prefix("--expand-")
    } else {
        None
    };
    let value = match expansion
        .or_else(|| name.strip_prefix("--"))
        .and_then(|name| long_option_value(name, curl, expansion.is_some()))
    {
        Some(value) => value,
        None if (inline.is_some() || name.starts_with('-')) && sensitive_name(name) => {
            Value::Secret
        }
        None if !curl && argument.starts_with("-p") && argument.len() > 2 => {
            return Some(OptionValue {
                name: "-p",
                inline: Some(("", argument.get(2..)?)),
                value: Value::Secret,
            });
        }
        None => return None,
    };
    Some(OptionValue {
        name,
        inline,
        value,
    })
}

fn long_option_value(name: &str, curl: bool, expanded: bool) -> Option<Value> {
    let authority = Value::Authority { curl };
    let common: [(&[&str], Value); 3] = [
        (&["header", "proxy-header"], Value::Header { expanded }),
        (
            &[
                "user",
                "proxy-user",
                "oauth2-bearer",
                "cookie",
                "cert",
                "proxy-cert",
                "pass",
                "proxy-pass",
                "ftp-account",
            ],
            Value::Secret,
        ),
        (&["proxy", "preproxy", "proxy1.0"], authority),
    ];
    let curl_only: [(&[&str], Value); 3] = [
        (
            &[
                "doh-url",
                "referer",
                "socks4",
                "socks4a",
                "socks5",
                "socks5-hostname",
                "url",
            ],
            authority,
        ),
        (
            &[
                "tlsuser",
                "tlspassword",
                "proxy-tlsuser",
                "proxy-tlspassword",
            ],
            Value::Secret,
        ),
        (&["variable"], Value::Variable),
    ];
    let options = common
        .into_iter()
        .chain(curl_only.into_iter().filter(|_| curl))
        .flat_map(|(names, value)| names.iter().map(move |&name| (name, value)));
    if let Some((_, value)) = options.clone().find(|&(option, _)| option == name) {
        return Some(value);
    }
    // Curl accepts unique prefixes. An exact non-secret option still wins
    // when its name is a prefix, as with --head and --header.
    if !curl || name.is_empty() || name == "head" {
        return None;
    }
    let mut prefixes = options.filter(|&(option, _)| option.starts_with(name));
    let (_, value) = prefixes.next()?;
    prefixes.next().is_none().then_some(value)
}

fn curl_short_option(argument: &str) -> Option<OptionValue<'_>> {
    for (index, option) in argument.char_indices().skip(1) {
        let value = match option {
            'u' | 'U' | 'b' | 'E' => Value::Secret,
            'H' => Value::Header { expanded: false },
            'e' | 'x' => Value::Authority { curl: true },
            // Only flag options can precede another short option in a group.
            '#' | ':' | '0' | '1' | '2' | '3' | '4' | '6' | 'a' | 'B' | 'f' | 'g' | 'G' | 'i'
            | 'I' | 'j' | 'J' | 'k' | 'l' | 'L' | 'M' | 'n' | 'N' | 'O' | 'p' | 'q' | 'R' | 's'
            | 'S' | 'v' | 'V' | 'Z' => continue,
            _ => return None,
        };
        let (name, rest) = argument.split_at(index + option.len_utf8());
        return Some(OptionValue {
            name,
            inline: (!rest.is_empty()).then_some(("", rest)),
            value,
        });
    }
    None
}

fn sensitive_name(name: &str) -> bool {
    if name == "-p" {
        return true;
    }
    let option = name.to_ascii_lowercase();
    ["token", "password", "passwd", "secret", "key"]
        .iter()
        .any(|word| {
            option.split(['-', '_', '.']).any(|part| part == *word) || option.ends_with(word)
        })
}

fn redact_header(argument: &str, expanded: bool) -> String {
    let sensitive = |line: &str| {
        let name = line.split(':').next().unwrap_or_default().trim();
        (expanded && name.contains("{{")) || is_sensitive_header(name) || sensitive_name(name)
    };
    let mut lines = argument.lines();
    if let Some(first) = lines.next()
        && sensitive(first)
    {
        return first
            .split_once(':')
            .filter(|(name, _)| !expanded || !name.contains("{{"))
            .map_or_else(
                || REDACTED_VALUE.to_owned(),
                |(name, _)| format!("{name}: {REDACTED_VALUE}"),
            );
    }
    // Header input can include multiple lines. If one of the later fields
    // contains credentials, mask the argument including any continuations.
    if lines.any(sensitive) {
        return REDACTED_VALUE.to_owned();
    }
    redact_urls(argument)
}

fn redact_authority(argument: &str, curl: bool) -> String {
    if curl && curl_url_may_expand_userinfo(argument) {
        return REDACTED_VALUE.to_owned();
    }
    if let Some(at) = userinfo_end(argument)
        && let Some(host) = argument.get(at..)
    {
        return format!("{REDACTED_VALUE}{host}");
    }
    redact_urls(argument)
}

fn curl_url_may_expand_userinfo(argument: &str) -> bool {
    // Variables can insert an @. Globs rearrange the literal text into
    // multiple authorities, so an existing @ cannot be treated as path data.
    argument.contains("{{")
        || (argument.contains('@')
            && (argument.contains('{')
                || argument.split('[').skip(1).any(|part| {
                    let Some((range, _)) = part.split_once(']') else {
                        return false;
                    };
                    let address = range.split_once('%').map_or(range, |(ip, _)| ip);
                    range.contains('-') && address.parse::<Ipv6Addr>().is_err()
                })))
}

fn redact_variable(argument: &str) -> String {
    if let Some((name, _)) = argument.split_once('=')
        && !name.contains('@')
    {
        return format!("{name}={REDACTED_VALUE}");
    }
    redact_urls(argument)
}

fn redact_urls(argument: &str) -> String {
    let mut result = String::new();
    let mut copied = 0;
    for (slash, _) in argument.match_indices('/') {
        if let Some(start) = slash_authority_start(argument, slash)
            && let Some(rest) = argument.get(start..)
            && let Some(at) = userinfo_end(rest)
            && let Some(prefix) = argument.get(copied..start)
        {
            result.push_str(prefix);
            result.push_str(REDACTED_VALUE);
            copied = start + at;
        }
    }
    result.push_str(argument.get(copied..).unwrap_or_default());
    result
}

fn slash_authority_start(argument: &str, slash: usize) -> Option<usize> {
    let (prefix, suffix) = argument.split_at_checked(slash)?;
    if let Some(scheme_and_boundary) = prefix.strip_suffix(':') {
        let boundary = scheme_and_boundary
            .trim_end_matches(|c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        let scheme = scheme_and_boundary.get(boundary.len()..)?;
        if !url_boundary(boundary)
            || !scheme
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            || scheme.eq_ignore_ascii_case("file")
        {
            return None;
        }
        // Curl permits one, two, or three slashes after the scheme.
        let authority = suffix.trim_start_matches('/');
        let slashes = suffix.len() - authority.len();
        return (1..=3).contains(&slashes).then_some(slash + slashes);
    }
    (url_boundary(prefix) && suffix.starts_with("//")).then_some(slash + 2)
}

fn url_boundary(prefix: &str) -> bool {
    prefix.chars().next_back().is_none_or(|previous| {
        matches!(previous, '=' | ',' | ';' | '|' | '(' | '[' | '"' | '\'')
            || previous.is_whitespace()
    })
}

fn userinfo_end(value: &str) -> Option<usize> {
    value
        .split(|c: char| matches!(c, '/' | '?' | '#') || c.is_ascii_whitespace())
        .next()?
        .rfind('@')
}

fn append_argument(result: &mut String, argument: &str) {
    if argument.is_empty()
        || argument.chars().any(|character| {
            character.is_whitespace() || matches!(character, '"' | '\\') || character.is_control()
        })
    {
        let _written = write!(result, "\"{}\"", argument.escape_debug());
    } else {
        result.push_str(argument);
    }
}

#[cfg(test)]
mod tests {
    use super::command;
    use std::path::Path;

    #[test]
    fn hides_sensitive_values_and_url_user_info() {
        let bytes = b"example\0--refresh-token=token123\0-p\0pass phrase\0--client_secret\0next-secret\0--apiKey=another-key\0-pattached\0token=assignment\0--url=https://user:p%40ss@site.test/a\0https://site.test/u@name\0--normal\0visible value\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "example --refresh-token=[REDACTED] -p [REDACTED] --client_secret [REDACTED] --apiKey=[REDACTED] -p[REDACTED] token=[REDACTED] --url=https://[REDACTED]@site.test/a https://site.test/u@name --normal \"visible value\""
            )
        );
    }

    #[test]
    fn hides_curl_credentials_in_separate_inline_and_grouped_options() {
        let bytes = b"/usr/bin/curl\0--user\0demo:review-password\0-U\0demo:proxy-password\0-uinline:another-password\0--proxy-user=demo:proxy-inline\0-fsSu\0group:credential\0-ooutput.txt\0-p\0https://public.test/\0-xproxy:secret@proxy.test:8080\0--oauth2-bearer\0review-bearer\0-bsession=value\0--proxy-pass\0passphrase\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "/usr/bin/curl --user [REDACTED] -U [REDACTED] -u[REDACTED] --proxy-user=[REDACTED] -fsSu [REDACTED] -ooutput.txt -p https://public.test/ -x[REDACTED]@proxy.test:8080 --oauth2-bearer [REDACTED] -b[REDACTED] --proxy-pass [REDACTED]"
            )
        );
    }

    #[test]
    fn hides_curl_headers_and_preserves_non_sensitive_headers() {
        let bytes = b"alias\0-H\0Authorization: Bearer review-token\0--header=X-Api-Key: review-api-key\0-sHProxy-Authorization: Basic review-proxy\0--proxy-header\0Cookie: review-cookie\0--header\0Accept: application/json\0--header\0Accept: */*\nX-Secret: review-extra\n review-continuation\0";
        assert_eq!(
            command(bytes, Some(Path::new("/usr/bin/curl"))).as_deref(),
            Some(
                "alias -H \"Authorization: [REDACTED]\" \"--header=X-Api-Key: [REDACTED]\" \"-sHProxy-Authorization: [REDACTED]\" --proxy-header \"Cookie: [REDACTED]\" --header \"Accept: application/json\" --header [REDACTED]"
            )
        );
        assert_eq!(
            command(b"python3\0-u\0script.py\0", None).as_deref(),
            Some("python3 -u script.py")
        );
    }

    #[test]
    fn hides_expanded_curl_authentication_and_header_values() {
        let bytes = b"curl\0--expand-header\0Authorization: Bearer fixture-token\0--expand-header=X-Api-Key: review-api-key\0--expand-proxy-header=Proxy-Authorization: Basic fixture-proxy\0--expand-user\0demo:fixture-password\0--expand-proxy-user=demo:fixture-proxy-password\0--expand-oauth2-bearer\0fixture-bearer\0--expand-header\0Accept: {{media}}\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "curl --expand-header \"Authorization: [REDACTED]\" \"--expand-header=X-Api-Key: [REDACTED]\" \"--expand-proxy-header=Proxy-Authorization: [REDACTED]\" --expand-user [REDACTED] --expand-proxy-user=[REDACTED] --expand-oauth2-bearer [REDACTED] --expand-header \"Accept: {{media}}\""
            )
        );
    }

    #[test]
    fn hides_curl_variable_contents_and_headers_with_expanded_names() {
        let bytes = b"curl\0--variable\0field=Authorization:\0--variable=value=fixtures-only\0--expand-header\0{{field}} Bearer fixture-token\0--expand-proxy-header={{field:trim}}: Basic fixture-proxy\0--expand-variable\0copy={{value}}-plaintext\0--variable\0from_file@fixture=example.txt\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "curl --variable field=[REDACTED] --variable=value=[REDACTED] --expand-header [REDACTED] --expand-proxy-header=[REDACTED] --expand-variable copy=[REDACTED] --variable from_file@fixture=example.txt"
            )
        );
    }

    #[test]
    fn hides_curl_userinfo_in_urls_with_an_optional_scheme() {
        let bytes = b"curl\0--expand-url\0demo:fixture-pass@localhost:8765/a\0--url=https://demo:fixture-pass@example.test/b\0demo:fixture-pass@localhost:8765/c\0--expand-url={{scheme}}//demo:fixture-pass@localhost:8765/d\0--expand-url\0http://demo:fixture-pass{{destination}}\0https://example.test/a@name\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "curl --expand-url [REDACTED]@localhost:8765/a --url=https://[REDACTED]@example.test/b [REDACTED]@localhost:8765/c --expand-url=[REDACTED] --expand-url [REDACTED] https://example.test/a@name"
            )
        );
    }

    #[test]
    fn hides_curl_authentication_and_headers_with_unique_prefixes() {
        let bytes = b"curl\0--head\0--oauth2-b\0fixture-token\0--proxy-us=demo:fixture-password\0--heade\0Authorization: Bearer fixture-header\0--proxy-head=X-Api-Key: fixture-key\0--user-agent\0sample-agent\0--proxy-tlspass\0fixture-tls\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "curl --head --oauth2-b [REDACTED] --proxy-us=[REDACTED] --heade \"Authorization: [REDACTED]\" \"--proxy-head=X-Api-Key: [REDACTED]\" --user-agent sample-agent --proxy-tlspass [REDACTED]"
            )
        );
    }

    #[test]
    fn hides_curl_userinfo_with_one_or_three_slashes_and_raw_quotes() {
        let bytes = b"curl\0http:/demo:fixture-one@localhost:8765/a@name\0http:///demo:fixture-three@localhost:8765/b\0http://demo:fix\"ture@localhost:8765/c\0http://example.test/path:/tag@name\0file:///fixture@name\0--url-list=http:/user:one@first.test/,http:///user:two@second.test/\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "curl http:/[REDACTED]@localhost:8765/a@name http:///[REDACTED]@localhost:8765/b http://[REDACTED]@localhost:8765/c http://example.test/path:/tag@name file:///fixture@name --url-list=http:/[REDACTED]@first.test/,http:///[REDACTED]@second.test/"
            )
        );
    }

    #[test]
    fn hides_userinfo_in_documented_curl_url_options() {
        let bytes = b"curl\0-xhttp:/demo:fixture@proxy.test:8080\0--preproxy=socks5:///demo:fixture@hop.test\0--socks5-hostname\0demo:fixture@socks.test\0--doh-url\0https://demo:fixture@dns.test/query\0-ehttp:///demo:fixture@origin.test/path@name\0--expand-url\0http:/demo:fixture@site.test/a\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "curl -xhttp:/[REDACTED]@proxy.test:8080 --preproxy=socks5:///[REDACTED]@hop.test --socks5-hostname [REDACTED]@socks.test --doh-url https://[REDACTED]@dns.test/query -ehttp:///[REDACTED]@origin.test/path@name --expand-url http:/[REDACTED]@site.test/a"
            )
        );
    }

    #[test]
    fn hides_entire_curl_globs_with_schemed_or_schemeless_credentials() {
        let bytes = b"curl\0{http://demo:glob-first@127.0.0.1:8765/first,http://demo:glob-second@127.0.0.1:8765/second}\0{demo:glob-first@127.0.0.1:8765/first,demo:glob-second@127.0.0.1:8765/second}\0--url={http:///demo:glob-first@127.0.0.1:8765/first,demo:glob-second@127.0.0.1:8765/second}\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some("curl [REDACTED] [REDACTED] --url=[REDACTED]")
        );
    }

    #[test]
    fn hides_curl_combined_globs_and_variables_in_url_options() {
        let bytes = b"curl\0--url\0http://demo:{secret,other}[1-2]@localhost:8765/path\0--socks5-hostname=demo:{fixture-one,fixture-two}@socks.test\0-xhttp://demo:fixture[a-b]@proxy.test:8080\0--expand-url\0{{credential_url}}\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "curl --url [REDACTED] --socks5-hostname=[REDACTED] -x[REDACTED] --expand-url [REDACTED]"
            )
        );
    }

    #[test]
    fn keeps_static_ipv6_and_globs_without_userinfo() {
        let bytes = b"curl\0http://[fe80::1%25some-interface]/path@name\0http://demo:fixture@[::1]/path@name\0http://example.test/{one,two}[1-2]\0http://example.test/a%7Bb%7D@name\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "curl http://[fe80::1%25some-interface]/path@name http://[REDACTED]@[::1]/path@name http://example.test/{one,two}[1-2] http://example.test/a%7Bb%7D@name"
            )
        );
    }

    #[test]
    fn hides_every_url_authority_with_userinfo_in_one_argument() {
        let bytes = b"example\0--url-list=https://first.test/,https://alice:review-password@second.test,https://bob:second-secret@third.test/path,https://plain.test/path@name\0--url=//carol:relative-secret@relative.test/\0https://eve,example:comma-secret@last.test/\0";
        assert_eq!(
            command(bytes, None).as_deref(),
            Some(
                "example --url-list=https://first.test/,https://[REDACTED]@second.test,https://[REDACTED]@third.test/path,https://plain.test/path@name --url=//[REDACTED]@relative.test/ https://[REDACTED]@last.test/"
            )
        );
    }
}
