use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::{Browser, Kind};

const CANDIDATES: &[(&str, &str, Kind)] = &[
    ("firefox", "Firefox", Kind::Firefox { snap: false }),
    ("firefox-esr", "Firefox ESR", Kind::Firefox { snap: false }),
    ("google-chrome", "Google Chrome", Kind::Chrome),
    ("google-chrome-stable", "Google Chrome", Kind::Chrome),
    ("google-chrome-beta", "Google Chrome Beta", Kind::Chrome),
    ("google-chrome-unstable", "Google Chrome Dev", Kind::Chrome),
    ("chromium", "Chromium", Kind::Chrome),
    ("chromium-browser", "Chromium", Kind::Chrome),
];

pub(super) fn installed() -> Vec<Browser> {
    discover(
        &std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>(),
    )
}

fn discover(paths: &[PathBuf]) -> Vec<Browser> {
    let mut seen = HashSet::new();
    CANDIDATES
        .iter()
        .filter_map(|&(command, name, kind)| {
            let executable = paths
                .iter()
                .filter(|path| path.is_absolute())
                .map(|path| path.join(command))
                .find(|path| {
                    fs::metadata(path).is_ok_and(|metadata| {
                        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                    })
                })?;
            let canonical = fs::canonicalize(&executable).ok()?;
            if !seen.insert(canonical.clone()) {
                return None;
            }
            let snap = executable.starts_with("/snap/bin")
                || canonical.starts_with("/snap")
                || is_snap_wrapper(&executable);
            // Chromium Snap uses a separate shared NSS store, which is not supported.
            if snap && kind == Kind::Chrome {
                return None;
            }
            if snap && !std::path::Path::new("/snap/firefox/current").exists() {
                return None;
            }
            Some(Browser {
                name: if snap { "Firefox (Snap)" } else { name },
                executable: if snap {
                    PathBuf::from("/snap/bin/firefox")
                } else {
                    executable
                },
                kind: if snap {
                    Kind::Firefox { snap: true }
                } else {
                    kind
                },
            })
        })
        .collect()
}

fn is_snap_wrapper(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut prefix = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(8192).read_to_end(&mut prefix))
        .is_ok_and(|_| {
            let text = String::from_utf8_lossy(&prefix);
            text.starts_with("#!") && (text.contains("/snap/bin/") || text.contains("exec snap "))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn discovers_executables_deduplicates_aliases_and_ignores_nonexecutables()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let firefox = directory.path().join("firefox");
        fs::write(&firefox, "#!/bin/sh\n")?;
        fs::set_permissions(&firefox, fs::Permissions::from_mode(0o700))?;
        symlink(&firefox, directory.path().join("firefox-esr"))?;
        fs::write(directory.path().join("google-chrome"), "not executable")?;
        fs::create_dir(directory.path().join("chromium"))?;
        let browsers = discover(&[directory.path().to_owned()]);
        assert_eq!(browsers.len(), 1);
        assert_eq!(browsers.first().map(Browser::name), Some("Firefox"));
        assert_eq!(discover(&[PathBuf::from("relative")]), []);
        Ok(())
    }
}
