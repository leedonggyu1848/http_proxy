//! Platform storage locations, resolved without changing the process environment.

use std::ffi::OsString;
use std::path::PathBuf;

/// Returns the application settings directory, ignoring invalid environment paths.
pub(crate) fn settings_directory(
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> Option<PathBuf> {
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
pub(crate) fn ca_directory(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let directory = macos_directory(&mut lookup);
    #[cfg(not(target_os = "macos"))]
    let directory = xdg_directory(&mut lookup, "XDG_DATA_HOME", ".local/share");
    directory.map(|directory| directory.join("ca"))
}

fn absolute_path(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|path| path.is_absolute())
}

#[cfg(any(test, not(target_os = "macos")))]
fn xdg_directory(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    variable: &str,
    fallback: &str,
) -> Option<PathBuf> {
    absolute_path(lookup(variable))
        .or_else(|| absolute_path(lookup("HOME")).map(|home| home.join(fallback)))
        .map(|base| base.join("http-inspector"))
}

#[cfg(any(test, target_os = "macos"))]
fn macos_directory(lookup: &mut impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    absolute_path(lookup("HOME"))
        .map(|home| home.join("Library/Application Support/http-inspector"))
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
        assert_eq!(settings_directory(environment(&[])), None);
        assert_eq!(ca_directory(environment(&[])), None);
    }
}
