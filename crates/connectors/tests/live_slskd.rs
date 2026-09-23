//! Live check of the managed slskd: download the pinned release, verify it,
//! start it with no Soulseek login, and talk to its API. Ignored by default.
//! `CD_SLSKD_ROOT=/tmp/x cargo test -p cd-connectors --test live_slskd -- --ignored --nocapture`
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use cd_connectors::http::Http;
use cd_connectors::slskd::install;
use cd_connectors::slskd::process::{Login, Managed, State};

#[test]
#[ignore = "downloads about 60 MB and runs slskd"]
fn managed_slskd_installs_starts_and_answers() {
    let root = std::path::PathBuf::from(std::env::var("CD_SLSKD_ROOT").expect("set CD_SLSKD_ROOT"));
    if !install::installed(&root) {
        let progress = AtomicU64::new(0);
        let bin = install::download(&root, &progress).unwrap();
        println!(
            "installed {} ({} bytes downloaded)",
            bin.display(),
            progress.into_inner()
        );
    }
    let staging = root.join("staging");
    let managed = Managed::new(root.clone(), Arc::new(Http::default()));
    let login = Login {
        username: String::new(),
        password: String::new(),
    };
    let started = std::time::Instant::now();
    let result = managed.start(&login, &staging);
    println!(
        "start: {result:?} after {:?}, state {:?}",
        started.elapsed(),
        managed.state()
    );
    result.unwrap();
    assert!(matches!(managed.state(), State::SignedOut(_)));
    let api = managed.api(Managed::downloads_dir(&staging)).unwrap();
    assert!(api.endpoint.starts_with("http://127.0.0.1:"));
    assert!(!api.logged_in().unwrap());
    // A wrong key is refused.
    let mut wrong = api.clone();
    wrong.api_key = "not-the-key-0000000000".into();
    assert!(wrong.logged_in().is_err());
    managed.stop();
    assert_eq!(managed.state(), State::Stopped);
}
