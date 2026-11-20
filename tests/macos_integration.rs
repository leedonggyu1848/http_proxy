#![cfg(target_os = "macos")]

use std::error::Error;
use std::time::{Duration, Instant};

use http_proxy::{
    Browser, BrowserEvent, CaInfo, CaTrustStatus, CaTrustStore, ProxyConfig, ProxyHandle,
};

#[tokio::test]
#[ignore = "Changes login keychain trust; may require interactive macOS authorization"]
async fn login_keychain_registration_lifecycle() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let ca = CaInfo::load_or_create(&directory.path().join("ca"))?;
    let store = CaTrustStore::for_current_user()?;
    assert_eq!(store.status(&ca).await?, CaTrustStatus::NotInstalled);
    let imported = tokio::process::Command::new("/usr/bin/security")
        .arg("add-certificates")
        .arg("-k")
        .arg(store.location())
        .arg(ca.certificate_path())
        .output()
        .await?;
    let untrusted = store.status(&ca).await;
    let installed = store.install(&ca).await;
    let installed_again = store.install(&ca).await;
    let checked = store.status(&ca).await;
    // Attempt cleanup even if registration or its verification failed.
    let removed = store.remove(&ca).await;
    assert!(imported.status.success());
    assert_eq!(untrusted?, CaTrustStatus::Untrusted);
    assert_eq!(installed?, CaTrustStatus::Trusted);
    assert_eq!(installed_again?, CaTrustStatus::Trusted);
    assert_eq!(checked?, CaTrustStatus::Trusted);
    assert_eq!(removed?, CaTrustStatus::NotInstalled);
    assert_eq!(store.remove(&ca).await?, CaTrustStatus::NotInstalled);
    Ok(())
}

#[tokio::test]
#[ignore = "Opens an installed Chrome inspection window on a graphical macOS session"]
async fn installed_chrome_launches_and_closes() -> Result<(), Box<dyn Error>> {
    let browser = Browser::installed()
        .into_iter()
        .find(|browser| browser.name() == "Google Chrome")
        .ok_or("Google Chrome is not installed")?;
    let proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut session = browser.open(proxy.local_addr(), None)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match session.try_receive() {
            Some(BrowserEvent::Opened) => break,
            Some(BrowserEvent::Finished(result)) => {
                return Err(format!("Browser exited: {result:?}").into());
            }
            None if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            None => return Err("Browser did not open within 20 seconds".into()),
        }
    }
    drop(session);
    proxy.stop()?;
    Ok(())
}
