//! Live Discogs discovery with the token saved in the keychain.
//! `cargo test -p cd-connectors --test live_discogs -- --ignored --nocapture`
use std::sync::Arc;

use cd_connectors::credentials::{Credential, Keychain, SecretStore};
use cd_connectors::discovery::discogs::Discogs;
use cd_connectors::discovery::{expand, Collector, DISCOGS_BUDGET};
use cd_connectors::http::Http;
use cd_core::adapters::Seed;
use cd_core::domain::SeedKind;

#[test]
#[ignore = "needs the user's Discogs token in the keychain and network access"]
fn discogs_expands_real_seeds() {
    // CD_DISCOGS_TOKEN avoids a keychain permission prompt when running unattended.
    let token = std::env::var("CD_DISCOGS_TOKEN")
        .ok()
        .or_else(|| Keychain.get(Credential::Discogs).unwrap())
        .expect("no Discogs token saved");
    let discogs = Discogs::new(token, Arc::new(Http::default()), DISCOGS_BUDGET);
    let seeds = [
        Seed {
            kind: SeedKind::Artist,
            value: "Kerri Chandler".into(),
        },
        Seed {
            kind: SeedKind::Label,
            value: "Underground Resistance".into(),
        },
    ];
    let mut out = Collector::default();
    let started = std::time::Instant::now();
    expand(&discogs, &seeds, 20, &mut out).unwrap();
    println!(
        "{} candidates, {} requests left, {:.1} s",
        out.len(),
        discogs.remaining(),
        started.elapsed().as_secs_f64()
    );
    for p in out.proposals.iter().take(12) {
        println!(
            "{} - {}{} | {} | {}",
            p.artist,
            p.title,
            p.mix.as_ref().map(|m| format!(" ({m})")).unwrap_or_default(),
            p.reasons[0],
            p.evidence[0].source_url.as_deref().unwrap_or("")
        );
    }
    assert!(out.len() >= 5);
    assert!(out.proposals.iter().all(|p| p.evidence[0]
        .source_url
        .as_deref()
        .unwrap_or("")
        .starts_with("https://www.discogs.com/")));
}
