//! Header fields whose values carry credentials and are never retained.

use http::{HeaderName, HeaderValue};

/// Value the capture keeps in place of a sensitive header's original value.
pub(crate) const REDACTED_VALUE: &str = "[REDACTED]";

/// Credential fields a client sends. Each is also a [sensitive header](is_sensitive_header).
const SENSITIVE_REQUEST_HEADERS: [&str; 3] = ["authorization", "proxy-authorization", "cookie"];
/// Credential fields a server sends.
const SENSITIVE_RESPONSE_HEADERS: [&str; 1] = ["set-cookie"];

/// Reports whether `name`, in any case, names a field whose value the capture
/// replaces with [`REDACTED_VALUE`].
pub(crate) fn is_sensitive_header(name: &str) -> bool {
    is_sensitive_request_header(name)
        || SENSITIVE_RESPONSE_HEADERS
            .iter()
            .any(|sensitive| name.eq_ignore_ascii_case(sensitive))
}

/// Reports whether `name`, in any case, names a credential field a client
/// sends, such as `Authorization` or `Cookie`.
pub(crate) fn is_sensitive_request_header(name: &str) -> bool {
    SENSITIVE_REQUEST_HEADERS
        .iter()
        .any(|sensitive| name.eq_ignore_ascii_case(sensitive))
}

/// Returns the bytes to retain for one header field: [`REDACTED_VALUE`] for
/// a sensitive field, otherwise the original value.
pub(crate) fn redacted_value<'a>(name: &HeaderName, value: &'a HeaderValue) -> &'a [u8] {
    if is_sensitive_header(name.as_str()) {
        REDACTED_VALUE.as_bytes()
    } else {
        value.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use http::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, PROXY_AUTHORIZATION, SET_COOKIE};

    use super::*;

    #[test]
    fn redacts_credential_fields_in_either_direction() {
        let secret = HeaderValue::from_static("secret");
        for name in [AUTHORIZATION, PROXY_AUTHORIZATION, COOKIE, SET_COOKIE] {
            assert_eq!(redacted_value(&name, &secret), b"[REDACTED]");
        }
        assert_eq!(redacted_value(&CONTENT_TYPE, &secret), b"secret");
    }

    #[test]
    fn matches_names_in_any_case() {
        assert!(is_sensitive_header("Set-Cookie"));
        assert!(is_sensitive_request_header("AUTHORIZATION"));
        assert!(!is_sensitive_request_header("set-cookie"));
        assert!(!is_sensitive_header("x-authorization"));
    }
}
