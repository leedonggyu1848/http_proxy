//! Commands answered without the daemon's records: the CA certificate and
//! the shell environment that routes programs through the proxy.

use std::time::{SystemTime, UNIX_EPOCH};

use http_inspector_core::https::CaInfo;
use http_inspector_core::settings::paths;
use serde_json::{Value, json};

use crate::error::{CliError, ErrorCode};

/// Loads, or creates, the local CA and describes its public certificate.
/// The private key never leaves the CA module.
pub(crate) fn ca_info() -> Result<Value, CliError> {
    let info = load_ca()?;
    Ok(json!({
        "certificate_path": info.certificate_path().to_string_lossy(),
        "fingerprint_sha256": info.fingerprint_sha256(),
        "valid_from": rfc3339(info.valid_from()),
        "valid_until": rfc3339(info.valid_until()),
    }))
}

fn load_ca() -> Result<CaInfo, CliError> {
    let directory = paths::ca_directory(|name| std::env::var_os(name)).ok_or_else(|| {
        CliError::new(
            ErrorCode::HttpsUnavailable,
            "No data directory for the HTTPS inspection CA",
        )
        .with_hint("set HOME or XDG_DATA_HOME")
    })?;
    CaInfo::load_or_create(&directory)
        .map_err(|error| CliError::new(ErrorCode::HttpsUnavailable, error.to_string()))
}

/// Proxy variables that programs read, upper and lower case: curl reads
/// only the lower-case `http_proxy`, most other tools read either.
const PROXY_VARIABLES: [&str; 4] = ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"];
/// Adds the CA to Node.js's built-in roots instead of replacing them.
const ADDED_CA_VARIABLES: [&str; 1] = ["NODE_EXTRA_CA_CERTS"];
/// Replace the whole CA bundle of OpenSSL, curl, and Python requests.
const REPLACING_CA_VARIABLES: [&str; 3] = ["SSL_CERT_FILE", "CURL_CA_BUNDLE", "REQUESTS_CA_BUNDLE"];

/// The `proxy env` document for a running proxy's `status`.
pub(crate) fn proxy_env(status: &Value, ca_bundle: bool) -> Result<Value, CliError> {
    let address = status
        .get("address")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CliError::new(ErrorCode::ProxyNotRunning, "The proxy is not running")
                .with_hint("run `http-proxy proxy start`")
        })?;
    let proxy_url = format!("http://{address}");
    let mut variables: Vec<Value> = PROXY_VARIABLES
        .iter()
        .map(|name| variable(name, &proxy_url, false))
        .collect();
    let https = status.get("https_inspection").and_then(Value::as_bool) == Some(true);
    let mut notes = Vec::new();
    if https {
        let certificate = load_ca()?.certificate_path().to_string_lossy().into_owned();
        variables.extend(
            ADDED_CA_VARIABLES
                .iter()
                .map(|name| variable(name, &certificate, false)),
        );
        if ca_bundle {
            variables.extend(
                REPLACING_CA_VARIABLES
                    .iter()
                    .map(|name| variable(name, &certificate, true)),
            );
            notes.push(
                "SSL_CERT_FILE, CURL_CA_BUNDLE, and REQUESTS_CA_BUNDLE replace the system CA bundle; \
                 HTTPS that bypasses the proxy fails certificate checks while they are set",
            );
        } else {
            notes.push("curl and Python need the CA too: pass --cacert, or rerun with --ca-bundle");
        }
    } else {
        notes.push("HTTPS inspection is off: HTTPS passes through as CONNECT tunnels");
    }
    Ok(json!({
        "proxy_url": proxy_url,
        "https_inspection": https,
        "variables": variables,
        "notes": notes,
    }))
}

fn variable(name: &str, value: &str, replaces_system_roots: bool) -> Value {
    json!({ "name": name, "value": value, "replaces_system_roots": replaces_system_roots })
}

/// Quotes `value` for a POSIX shell.
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Formats a time as UTC RFC 3339, such as `2026-11-27T09:30:00Z`.
fn rfc3339(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

/// Converts days since 1970-01-01 to a proleptic Gregorian date
/// (Howard Hinnant's `civil_from_days`).
const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "indexing a serde_json value by key returns null instead of panicking"
)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn formats_utc_dates() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_661);
        assert_eq!(rfc3339(leap), "2000-02-29T01:01:01Z");
        let later = UNIX_EPOCH + Duration::from_secs(1_795_737_659);
        assert_eq!(rfc3339(later), "2026-11-27T00:00:59Z");
    }

    #[test]
    fn quotes_for_posix_shells() {
        assert_eq!(
            shell_quote("http://127.0.0.1:8080"),
            "'http://127.0.0.1:8080'"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn exports_proxy_variables_without_ca_when_inspection_is_off() -> Result<(), CliError> {
        let env = proxy_env(
            &json!({ "address": "127.0.0.1:8080", "https_inspection": false }),
            true,
        )?;
        let names: Vec<&str> = env["variables"]
            .as_array()
            .map_or_else(Vec::new, |variables| {
                variables
                    .iter()
                    .filter_map(|v| v["name"].as_str())
                    .collect()
            });
        assert_eq!(names, PROXY_VARIABLES);
        assert_eq!(env["proxy_url"], "http://127.0.0.1:8080");
        let stopped = proxy_env(&json!({ "address": null }), false);
        assert_eq!(
            stopped.err().map(|error| error.code),
            Some(ErrorCode::ProxyNotRunning)
        );
        Ok(())
    }
}
