//! Commands answered without the daemon's records: the CA certificate and
//! the shell environment that routes programs through the proxy.

use std::time::{SystemTime, UNIX_EPOCH};

use http_inspector_core::https::CaInfo;
use http_inspector_core::settings::paths;
use http_inspector_core::shell::{
    BUNDLE_VARIABLES, CaBundle, Change, NoProxy, ShellEnvironment, SystemRoots,
};
use serde_json::{Value, json};

use crate::control::ControlPath;
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
    CaInfo::load_or_create(&ca_directory()?)
        .map_err(|error| CliError::new(ErrorCode::HttpsUnavailable, error.to_string()))
}

/// The CA directory from `XDG_DATA_HOME` or `HOME`.
pub(crate) fn ca_directory() -> Result<std::path::PathBuf, CliError> {
    paths::ca_directory(|name| std::env::var_os(name)).ok_or_else(|| {
        CliError::new(
            ErrorCode::HttpsUnavailable,
            "No data directory for the HTTPS inspection CA",
        )
        .with_hint("set HOME or XDG_DATA_HOME")
    })
}

/// Reads the existing CA's public certificate for trust settings. The
/// private key file is never opened and no CA is created, so a missing or
/// mismatched CA directory fails instead of trusting a CA nobody uses.
pub(crate) fn load_public_ca(directory: &std::path::Path) -> Result<CaInfo, CliError> {
    CaInfo::load_public(directory).map_err(|error| {
        CliError::new(ErrorCode::HttpsUnavailable, error.to_string()).with_hint(
            "start HTTPS inspection with the same XDG_DATA_HOME, \
             such as `http-proxy proxy set --https-inspection on`",
        )
    })
}

/// The `proxy env` document for a running proxy's `status`. With
/// `ca_bundle`, the combined bundle is written to the control directory,
/// where the daemon removes it on `proxy stop`.
pub(crate) fn proxy_env(
    status: &Value,
    ca_bundle: bool,
    control: &ControlPath,
) -> Result<Value, CliError> {
    let proxy = crate::shell::proxy_address(status)?;
    let environment = ShellEnvironment::new(proxy, NoProxy::Keep).map_err(|error| {
        CliError::new(ErrorCode::ProxyNotRunning, error.to_string())
            .with_hint("run `http-proxy proxy start`")
    })?;
    let https = status.get("https_inspection").and_then(Value::as_bool) == Some(true);
    let mut notes = Vec::new();
    let environment = if https {
        let ca = load_public_ca(&ca_directory()?)?;
        let bundle = if ca_bundle {
            control.prepare_directory()?;
            let bundle = CaBundle::write_shared(control.directory(), &ca)
                .map_err(|error| CliError::new(ErrorCode::HttpsUnavailable, error.to_string()))?;
            notes.push(if bundle.system_roots() == &SystemRoots::Missing {
                "No system CA bundle was found, so the bundle holds only the local CA: \
                 HTTPS that bypasses the proxy fails certificate checks while it is set"
            } else {
                "The bundle holds the system roots and the local CA; `proxy stop` removes it"
            });
            Some(bundle)
        } else {
            notes.push(
                "curl and Python need the CA too: rerun with --ca-bundle, \
                 or use `http-proxy shell` or `http-proxy exec`",
            );
            None
        };
        environment.with_ca(&ca, bundle)
    } else {
        notes.push("HTTPS inspection is off: HTTPS passes through as CONNECT tunnels");
        environment
    };
    let replaces = environment
        .bundle()
        .is_some_and(|bundle| bundle.system_roots() == &SystemRoots::Missing);
    let variables: Vec<Value> = environment
        .changes()
        .into_iter()
        .filter_map(|change| match change {
            Change::Set(name, value) => Some(json!({
                "name": name,
                "value": value.to_string_lossy(),
                "replaces_system_roots": replaces && BUNDLE_VARIABLES.contains(&name),
            })),
            Change::Remove(_) => None,
        })
        .collect();
    Ok(json!({
        "proxy_url": format!("http://{proxy}"),
        "https_inspection": https,
        "variables": variables,
        "notes": notes,
    }))
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

    use http_inspector_core::shell::PROXY_VARIABLES;

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
        let control = ControlPath::in_directory(std::path::PathBuf::from("/nonexistent"));
        let env = proxy_env(
            &json!({ "address": "127.0.0.1:8080", "https_inspection": false }),
            true,
            &control,
        )?;
        let names: Vec<&str> = env["variables"]
            .as_array()
            .map_or_else(Vec::new, |variables| {
                variables
                    .iter()
                    .filter_map(|v| v["name"].as_str())
                    .collect()
            });
        assert_eq!(names.first(), Some(&"HTTP_INSPECTOR_PROXY"));
        assert_eq!(names.get(1..), Some(&PROXY_VARIABLES[..]));
        assert_eq!(env["proxy_url"], "http://127.0.0.1:8080");
        let stopped = proxy_env(&json!({ "address": null }), false, &control);
        assert_eq!(
            stopped.err().map(|error| error.code),
            Some(ErrorCode::ProxyNotRunning)
        );
        Ok(())
    }
}
