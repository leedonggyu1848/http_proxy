//! Platform storage locations, resolved without changing the process environment.

use std::ffi::OsString;
use std::path::PathBuf;

/// Directory name this application uses under each platform base directory.
const APP_DIRECTORY: &str = "http-inspector";

/// Returns the application settings directory, ignoring invalid environment paths.
pub fn settings_directory(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos_directory(&mut lookup)
    }
    #[cfg(not(target_os = "macos"))]
    {
        xdg_directory(&mut lookup, "XDG_CONFIG_HOME", ".config")
    }
}

/// Returns the CA directory, ignoring invalid environment paths.
pub fn ca_directory(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let directory = macos_directory(&mut lookup);
    #[cfg(not(target_os = "macos"))]
    let directory = xdg_directory(&mut lookup, "XDG_DATA_HOME", ".local/share");
    directory.map(|directory| directory.join("ca"))
}

/// Returns the body store root: a cache directory, because `/tmp` is often
/// RAM-backed and too small for video, and the bodies are disposable.
pub fn body_store_directory(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let directory =
        home_directory(&mut lookup).map(|home| home.join("Library/Caches").join(APP_DIRECTORY));
    #[cfg(not(target_os = "macos"))]
    let directory = xdg_directory(&mut lookup, "XDG_CACHE_HOME", ".cache");
    directory.map(|directory| directory.join("bodies"))
}

/// Returns the user's home directory from `HOME` when it is an absolute path.
pub fn home_directory(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    absolute_path(lookup("HOME"))
}

/// Returns the user data base directory shared by applications: an absolute
/// `XDG_DATA_HOME`, otherwise `$HOME/.local/share`.
pub fn data_home(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    xdg_base(&mut lookup, "XDG_DATA_HOME", ".local/share")
}

fn absolute_path(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|path| path.is_absolute())
}

fn xdg_base(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    variable: &str,
    fallback: &str,
) -> Option<PathBuf> {
    absolute_path(lookup(variable))
        .or_else(|| home_directory(&mut *lookup).map(|home| home.join(fallback)))
}

#[cfg(any(test, not(target_os = "macos")))]
fn xdg_directory(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    variable: &str,
    fallback: &str,
) -> Option<PathBuf> {
    xdg_base(lookup, variable, fallback).map(|base| base.join(APP_DIRECTORY))
}

#[cfg(any(test, target_os = "macos"))]
fn macos_directory(lookup: &mut impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    home_directory(lookup).map(|home| home.join("Library/Application Support").join(APP_DIRECTORY))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment<'a>(values: &'a [(&str, &str)]) -> impl FnMut(&str) -> Option<OsString> + 'a {
        move |name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.into())
        }
    }

    #[test]
    fn xdg_prefers_absolute_override_then_home() {
        for (variable, fallback) in [
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_CACHE_HOME", ".cache"),
        ] {
            for override_value in [None, Some(""), Some("relative"), Some("/custom")] {
                let mut values = vec![("HOME", "/home/test")];
                if let Some(value) = override_value {
                    values.push((variable, value));
                }
                let expected = if override_value == Some("/custom") {
                    PathBuf::from("/custom/http-inspector")
                } else {
                    PathBuf::from("/home/test")
                        .join(fallback)
                        .join("http-inspector")
                };
                assert_eq!(
                    xdg_directory(&mut environment(&values), variable, fallback),
                    Some(expected)
                );
            }
            for invalid in ["", "relative"] {
                assert_eq!(
                    xdg_directory(
                        &mut environment(&[(variable, invalid), ("HOME", invalid)]),
                        variable,
                        fallback
                    ),
                    None
                );
            }
            assert_eq!(
                xdg_directory(&mut environment(&[]), variable, fallback),
                None
            );
            assert_eq!(
                xdg_directory(
                    &mut environment(&[(variable, "/custom")]),
                    variable,
                    fallback
                ),
                Some(PathBuf::from("/custom/http-inspector"))
            );
        }
    }

    #[test]
    fn macos_uses_only_absolute_home() {
        assert_eq!(
            macos_directory(&mut environment(&[
                ("HOME", "/Users/test"),
                ("XDG_CONFIG_HOME", "/ignored")
            ])),
            Some(PathBuf::from(
                "/Users/test/Library/Application Support/http-inspector"
            ))
        );
        for home in ["", "relative"] {
            assert_eq!(macos_directory(&mut environment(&[("HOME", home)])), None);
        }
        assert_eq!(macos_directory(&mut environment(&[])), None);
    }

    #[test]
    fn public_paths_keep_platform_defaults() {
        let values = [("HOME", "/home/test")];
        #[cfg(target_os = "macos")]
        let base = PathBuf::from("/home/test/Library/Application Support/http-inspector");
        #[cfg(not(target_os = "macos"))]
        let base = PathBuf::from("/home/test/.config/http-inspector");
        assert_eq!(settings_directory(environment(&values)), Some(base));
        #[cfg(target_os = "macos")]
        let ca = PathBuf::from("/home/test/Library/Application Support/http-inspector/ca");
        #[cfg(not(target_os = "macos"))]
        let ca = PathBuf::from("/home/test/.local/share/http-inspector/ca");
        assert_eq!(ca_directory(environment(&values)), Some(ca));
        #[cfg(target_os = "macos")]
        let bodies = PathBuf::from("/home/test/Library/Caches/http-inspector/bodies");
        #[cfg(not(target_os = "macos"))]
        let bodies = PathBuf::from("/home/test/.cache/http-inspector/bodies");
        assert_eq!(body_store_directory(environment(&values)), Some(bodies));
        assert_eq!(settings_directory(environment(&[])), None);
        assert_eq!(ca_directory(environment(&[])), None);
        assert_eq!(body_store_directory(environment(&[])), None);
    }

    /// Every `XDG_DATA_HOME`/`HOME` combination keeps the locations used
    /// before these rules moved out of the UI.
    #[test]
    fn data_locations_match_previous_values_for_every_environment() {
        let homes = [None, Some(""), Some("relative"), Some("/home/test")];
        let data_homes = [None, Some(""), Some("relative"), Some("/data")];
        for home in homes {
            for data_home_value in data_homes {
                let mut values = Vec::new();
                if let Some(home) = home {
                    values.push(("HOME", home));
                }
                if let Some(data) = data_home_value {
                    values.push(("XDG_DATA_HOME", data));
                }
                let absolute_home = home.filter(|home| home.starts_with('/'));
                let expected_base = if data_home_value == Some("/data") {
                    Some(PathBuf::from("/data"))
                } else {
                    absolute_home.map(|home| PathBuf::from(home).join(".local/share"))
                };
                assert_eq!(data_home(environment(&values)), expected_base, "{values:?}");
                assert_eq!(
                    home_directory(environment(&values)),
                    absolute_home.map(PathBuf::from),
                    "{values:?}"
                );
                #[cfg(not(target_os = "macos"))]
                let expected_ca = expected_base.map(|base| base.join("http-inspector/ca"));
                #[cfg(target_os = "macos")]
                let expected_ca = absolute_home.map(|home| {
                    PathBuf::from(home).join("Library/Application Support/http-inspector/ca")
                });
                assert_eq!(
                    ca_directory(environment(&values)),
                    expected_ca,
                    "{values:?}"
                );
            }
        }
    }
}
