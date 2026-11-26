use super::predicate::Predicate;
use super::{FieldTerm, FilterDiagnostic, FilterKey, FilterQuery, TextTerm};

pub(super) fn query(input: &str) -> FilterQuery {
    let mut result = FilterQuery::default();
    for (span, closed) in spans(input, char::is_whitespace) {
        let Some(term) = input.get(span.clone()).filter(|term| !term.is_empty()) else {
            continue;
        };
        if let Err(message) = parse_term(&mut result, term, closed) {
            result.diagnostics.push(FilterDiagnostic { span, message });
        }
    }
    result
}

fn parse_term(query: &mut FilterQuery, term: &str, closed: bool) -> Result<(), String> {
    if !closed {
        return Err("Unclosed quote".into());
    }
    let (term, excluded) = term
        .strip_prefix('-')
        .filter(|rest| !rest.is_empty())
        .map_or((term, false), |rest| (rest, true));
    if let Some((key, values)) = term.split_once(':')
        && key_token(key, values)
    {
        if ["body", "req-body", "res-body"]
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(key))
        {
            return Err(format!("Reserved filter key `{key}`"));
        }
        let Some((_, field)) = FilterKey::ALL
            .into_iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
        else {
            return Err(format!("Unknown filter key `{key}`"));
        };
        let alternatives = spans(values, |c| c == ',')
            .map(|(span, _)| {
                Predicate::parse(field, &unquote(values.get(span).unwrap_or_default()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        query.fields.push(FieldTerm {
            alternatives,
            excluded,
        });
    } else {
        let text = unquote(term);
        if text.is_empty() {
            return Err("Empty search term".into());
        }
        query.text.push(TextTerm { text, excluded });
    }
    Ok(())
}

fn key_token(key: &str, value: &str) -> bool {
    let known = FilterKey::ALL
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case(key))
        || ["body", "req-body", "res-body"]
            .iter()
            .any(|name| name.eq_ignore_ascii_case(key));
    if known {
        return true;
    }
    let port = value.split(['/', '?', '#']).next().unwrap_or_default();
    !key.is_empty()
        && key.bytes().all(|c| c.is_ascii_alphabetic() || c == b'-')
        && !value.starts_with("//")
        && (port.is_empty() || !port.bytes().all(|c| c.is_ascii_digit()))
}

/// Emits ranges separated by delimiters outside double quotes.
fn spans(
    input: &str,
    separator: impl Fn(char) -> bool,
) -> impl Iterator<Item = (std::ops::Range<usize>, bool)> {
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    input
        .char_indices()
        .map(Some)
        .chain(std::iter::once(None))
        .filter_map(move |item| {
            let Some((index, c)) = item else {
                return Some((start..input.len(), !quoted));
            };
            if std::mem::take(&mut escaped) {
                return None;
            }
            match c {
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                _ if !quoted && separator(c) => {
                    let range = start..index;
                    start = index + c.len_utf8();
                    return Some((range, true));
                }
                _ => {}
            }
            None
        })
}

fn unquote(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' if quoted && matches!(chars.peek(), Some('"' | '\\')) => {
                output.extend(chars.next());
            }
            _ => output.push(c),
        }
    }
    output
}
