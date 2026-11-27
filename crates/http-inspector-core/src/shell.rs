//! The environment of an inspection shell.
//!
//! Proxy variables send a shell's programs through the proxy. While HTTPS
//! is decrypted, CA variables make them trust the local CA without losing
//! the system roots.
//!
//! The desktop app and the command line build the environment here, so a
//! terminal opened from the app and `http-proxy shell` set the same values.
//! Nothing outside the child process changes: no system trust store, no
//! firewall rule, and no shell startup file.

mod bundle;

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

pub use bundle::{BUNDLE_FILE, CaBundle, CaBundleError, SystemRoots, remove_shared};

use crate::https::CaInfo;

/// Holds the proxy address (`127.0.0.1:8080`) in an inspection shell, so a
/// prompt or script can tell that it runs inside one.
pub const MARKER_VARIABLE: &str = "HTTP_INSPECTOR_PROXY";

/// Proxy variables, upper and lower case: curl reads only the lower-case
/// `http_proxy`, while most other tools read either.
pub const PROXY_VARIABLES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// Proxy exclusion lists, removed by [`NoProxy::Clear`].
pub const NO_PROXY_VARIABLES: [&str; 2] = ["NO_PROXY", "no_proxy"];

/// Adds the local CA certificate to Node.js's built-in roots.
pub const ADDED_CA_VARIABLES: [&str; 1] = ["NODE_EXTRA_CA_CERTS"];

/// Replace a program's whole CA bundle: OpenSSL, curl, Python requests,
/// git, the AWS CLI, and Deno. They name the combined bundle.
pub const BUNDLE_VARIABLES: [&str; 6] = [
    "SSL_CERT_FILE",
    "CURL_CA_BUNDLE",
    "REQUESTS_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "DENO_CERT",
];

/// What happens to inherited `NO_PROXY` and `no_proxy`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NoProxy {
    /// Remove them, so requests to local test servers are captured too.
    /// Requests to the proxy's own address are refused by the proxy.
    #[default]
    Clear,
    /// Keep the inherited values; listed hosts bypass the proxy.
    Keep,
}

/// Reason an inspection environment cannot be built.
#[derive(Debug, thiserror::Error)]
pub enum ShellEnvironmentError {
    #[error("An inspection shell requires a listening loopback proxy")]
    InvalidProxyAddress,
    #[error(transparent)]
    Bundle(#[from] CaBundleError),
}

/// One change to a child's environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Change {
    /// Set the variable to the value.
    Set(&'static str, OsString),
    /// Remove the variable.
    Remove(&'static str),
}

/// The CA part of the environment.
#[derive(Debug)]
struct CaVariables {
    certificate: PathBuf,
    bundle: Option<CaBundle>,
}

/// Variables for programs run through the proxy. Owns the CA bundle, so
/// keep it alive while the programs run.
#[derive(Debug)]
pub struct ShellEnvironment {
    proxy: SocketAddr,
    no_proxy: NoProxy,
    ca: Option<CaVariables>,
}

impl ShellEnvironment {
    /// Proxy variables for `proxy`, which must be a loopback listener.
    pub const fn new(proxy: SocketAddr, no_proxy: NoProxy) -> Result<Self, ShellEnvironmentError> {
        if !proxy.ip().is_loopback() || proxy.port() == 0 {
            return Err(ShellEnvironmentError::InvalidProxyAddress);
        }
        Ok(Self {
            proxy,
            no_proxy,
            ca: None,
        })
    }

    /// Adds the CA variables for decrypted HTTPS, with a temporary bundle
    /// in a new private directory under `parent` (see
    /// [`private_parent`]). The bundle is removed when `self` drops.
    pub fn with_temporary_bundle(
        self,
        ca: &CaInfo,
        parent: &Path,
    ) -> Result<Self, ShellEnvironmentError> {
        let bundle = CaBundle::create_temporary(parent, ca)?;
        Ok(self.with_ca(ca, Some(bundle)))
    }

    /// Adds the CA variables. Without a bundle only
    /// [`ADDED_CA_VARIABLES`] are set.
    #[must_use]
    pub fn with_ca(mut self, ca: &CaInfo, bundle: Option<CaBundle>) -> Self {
        self.ca = Some(CaVariables {
            certificate: ca.certificate_path().to_owned(),
            bundle,
        });
        self
    }

    /// The proxy address the variables name.
    #[must_use]
    pub const fn proxy(&self) -> SocketAddr {
        self.proxy
    }

    /// Whether the CA variables are set.
    #[must_use]
    pub const fn trusts_local_ca(&self) -> bool {
        self.ca.is_some()
    }

    /// The bundle the [`BUNDLE_VARIABLES`] name, if any.
    #[must_use]
    pub fn bundle(&self) -> Option<&CaBundle> {
        self.ca.as_ref().and_then(|ca| ca.bundle.as_ref())
    }

    /// Every change, in a stable order.
    #[must_use]
    pub fn changes(&self) -> Vec<Change> {
        let url = OsString::from(format!("http://{}", self.proxy));
        let mut changes = vec![Change::Set(
            MARKER_VARIABLE,
            OsString::from(self.proxy.to_string()),
        )];
        changes.extend(
            PROXY_VARIABLES
                .iter()
                .map(|name| Change::Set(name, url.clone())),
        );
        if self.no_proxy == NoProxy::Clear {
            changes.extend(NO_PROXY_VARIABLES.iter().map(|name| Change::Remove(name)));
        }
        if let Some(ca) = &self.ca {
            changes.extend(
                ADDED_CA_VARIABLES
                    .iter()
                    .map(|name| Change::Set(name, ca.certificate.clone().into_os_string())),
            );
            if let Some(bundle) = &ca.bundle {
                let path = bundle.path().as_os_str();
                changes.extend(
                    BUNDLE_VARIABLES
                        .iter()
                        .map(|name| Change::Set(name, path.to_owned())),
                );
            }
        }
        changes
    }

    /// Applies [`Self::changes`] to `command`.
    pub fn apply(&self, command: &mut std::process::Command) {
        for change in self.changes() {
            match change {
                Change::Set(name, value) => {
                    command.env(name, value);
                }
                Change::Remove(name) => {
                    command.env_remove(name);
                }
            }
        }
    }
}

/// The parent directory for temporary bundles.
///
/// An absolute `XDG_RUNTIME_DIR` is private to the user and cleared at
/// logout; otherwise the system temporary directory is used. The bundle's
/// own directory is mode 0700 either way.
#[must_use]
pub fn private_parent(mut lookup: impl FnMut(&str) -> Option<OsString>) -> PathBuf {
    lookup("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn set(changes: &[Change], name: &str) -> Option<OsString> {
        changes.iter().find_map(|change| match change {
            Change::Set(set, value) if *set == name => Some(value.clone()),
            _ => None,
        })
    }

    #[test]
    fn proxy_variables_name_the_proxy_and_clear_no_proxy_by_default() -> TestResult {
        let proxy = "127.0.0.1:18080".parse()?;
        let changes = ShellEnvironment::new(proxy, NoProxy::default())?.changes();
        for name in PROXY_VARIABLES {
            assert_eq!(
                set(&changes, name),
                Some(OsString::from("http://127.0.0.1:18080"))
            );
        }
        assert_eq!(
            set(&changes, MARKER_VARIABLE),
            Some(OsString::from("127.0.0.1:18080"))
        );
        assert!(changes.contains(&Change::Remove("NO_PROXY")));
        assert!(changes.contains(&Change::Remove("no_proxy")));
        assert!(set(&changes, "SSL_CERT_FILE").is_none());
        let kept = ShellEnvironment::new(proxy, NoProxy::Keep)?.changes();
        assert!(
            !kept
                .iter()
                .any(|change| matches!(change, Change::Remove(_)))
        );
        assert!(ShellEnvironment::new("192.0.2.1:8080".parse()?, NoProxy::Clear).is_err());
        assert!(ShellEnvironment::new("127.0.0.1:0".parse()?, NoProxy::Clear).is_err());
        Ok(())
    }

    #[test]
    fn ca_variables_name_the_certificate_and_bundle_never_the_identity() -> TestResult {
        let root = tempfile::tempdir()?;
        let ca = CaInfo::load_or_create(&root.path().join("ca"))?;
        let environment = ShellEnvironment::new("127.0.0.1:18080".parse()?, NoProxy::Clear)?
            .with_temporary_bundle(&ca, root.path())?;
        let changes = environment.changes();
        assert_eq!(
            set(&changes, "NODE_EXTRA_CA_CERTS"),
            Some(ca.certificate_path().as_os_str().to_owned())
        );
        let bundle = environment.bundle().ok_or("no bundle")?.path().to_owned();
        for name in BUNDLE_VARIABLES {
            assert_eq!(set(&changes, name), Some(bundle.clone().into_os_string()));
        }
        for change in &changes {
            if let Change::Set(_, value) = change {
                assert!(!value.to_string_lossy().contains("ca-identity"));
            }
        }
        let mut command = std::process::Command::new("env");
        environment.apply(&mut command);
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == "NO_PROXY" && value.is_none())
        );
        drop(environment);
        assert!(!bundle.exists());
        Ok(())
    }

    #[test]
    fn private_parent_prefers_an_existing_absolute_runtime_directory() -> TestResult {
        let runtime = tempfile::tempdir()?;
        let value = runtime.path().as_os_str().to_owned();
        assert_eq!(private_parent(|_| Some(value.clone())), runtime.path());
        assert_eq!(
            private_parent(|_| Some(OsString::from("relative"))),
            std::env::temp_dir()
        );
        Ok(())
    }
}
