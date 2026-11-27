//! Command arguments are sanitized before they leave the process lookup.

use std::fmt::Write as _;

const REDACTED: &str = "[REDACTED]";

pub(super) fn command(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let mut result = String::new();
    let mut redact_next = false;
    for argument in bytes
        .strip_suffix(&[0])
        .unwrap_or(bytes)
        .split(|&byte| byte == 0)
    {
        let argument = String::from_utf8_lossy(argument);
        let redacted = if std::mem::take(&mut redact_next) {
            REDACTED.to_owned()
        } else if let Some((name, _)) = argument.split_once('=')
            && sensitive_name(name)
        {
            format!("{name}={REDACTED}")
        } else if argument.starts_with("-p") && argument.len() > 2 {
            format!("-p{REDACTED}")
        } else if argument.starts_with('-') && sensitive_name(&argument) {
            redact_next = true;
            argument.into_owned()
        } else {
            strip_url_userinfo(&argument)
        };
        if !result.is_empty() {
            result.push(' ');
        }
        append_argument(&mut result, &redacted);
    }
    Some(result)
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

fn strip_url_userinfo(argument: &str) -> String {
    let mut result = String::new();
    for word in argument.split_inclusive(char::is_whitespace) {
        let authority_start = word
            .find("://")
            .map(|i| i + 3)
            .or_else(|| word.starts_with("//").then_some(2));
        if let Some(start) = authority_start
            && let (Some(prefix), Some(rest)) = (word.get(..start), word.get(start..))
        {
            let end = rest
                .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
                .unwrap_or(rest.len());
            if let Some(authority) = rest.get(..end)
                && let Some(at) = authority.rfind('@')
                && let Some(host) = rest.get(at..)
            {
                result.push_str(prefix);
                result.push_str(REDACTED);
                result.push_str(host);
                continue;
            }
        }
        result.push_str(word);
    }
    result
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
    #[test]
    fn hides_sensitive_values_and_url_user_info() {
        let bytes = b"example\0--refresh-token=token123\0-p\0pass phrase\0--client_secret\0next-secret\0--apiKey=another-key\0-pattached\0token=assignment\0--url=https://user:p%40ss@site.test/a\0https://site.test/u@name\0--normal\0visible value\0";
        assert_eq!(
            super::command(bytes).as_deref(),
            Some(
                "example --refresh-token=[REDACTED] -p [REDACTED] --client_secret [REDACTED] --apiKey=[REDACTED] -p[REDACTED] token=[REDACTED] --url=https://[REDACTED]@site.test/a https://site.test/u@name --normal \"visible value\""
            )
        );
    }
}
