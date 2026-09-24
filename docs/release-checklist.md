# Release checklist

Issue #21. How a release is built, what must be true before it is published, and the validation report that goes with it.

## Building

1. Set the version in `Cargo.toml` (`[workspace.package]`) and `app/src-tauri/tauri.conf.json`. They must match.
2. Merge to `main` with CI green.
3. Tag `vX.Y.Z` and push the tag. `.github/workflows/release.yml` then:
   - checks that the tag matches both version fields;
   - runs the Rust tests on each platform;
   - builds installers for macOS Apple Silicon and Intel (`.dmg`), Windows x64 (`.msi` and `.exe`) and Linux x64 (`.deb`, `.rpm`, `.AppImage`, built on Ubuntu 22.04 for an older glibc);
   - launches each build with a throwaway data folder (`scripts/smoke-launch.sh`);
   - attaches the installers and `SHA256SUMS.txt` to a **draft** release.
4. Publish the draft only after the gates and clean-machine checks below pass.

**Signing:**
- macOS builds are signed and notarised when the repository has the secrets `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID`. Without them the build is unsigned, and Gatekeeper asks users to open it from the context menu.
- Windows builds are unsigned, so SmartScreen warns on first run.

State both in the release notes until signing is set up.

## Release gates (product spec, section 6)

| Gate | Evidence |
| --- | --- |
| Local core and recovery checks pass | CI on every platform. Crash and restart cases: `jobs::tests::quit_parks_running_jobs_and_start_resumes_them`, the `crash_*` cases in `archive`, `library` and the pipeline, `review::tests::restart_mid_pipeline_resumes_without_repeating_downloads`, `worker::a_crash_is_contained` and `worker::playback_continues_while_the_worker_crashes` |
| Validated matching before unattended acquisition | `docs/acquisition-calibration.md` (runs in CI); unattended downloads are off by default and ambiguous results always wait for the user (`acquisition::tests::ambiguous_results_wait_for_the_user_even_when_unattended`) |
| Pinned analysis versions before shared uploads | `protocol_registry::shared_model_matches_the_pinned_analysis_model`; the model registry refuses unknown or unpinned models (`unknown_models_and_unpinned_checksums_are_refused`) |
| Account isolation before private backup | crate-digger-service `backups_belong_only_to_their_account`, and the Firestore rules test that refuses direct clients |
| Tested exports | `export::tests` (M3U8 and Rekordbox XML). The Rekordbox import on a real Rekordbox install is a clean-machine check below |
| Target-platform compatibility recorded | The validation report below |

## Provider and service outages

Local features never wait for a network service:
- import, library, review, ratings, playlists, playback, analysis and export;
- the Rust tests run with networking removed on Linux, and offline on macOS and Windows.

Behaviour of each connection when it fails:
- **Discovery source:** a run records "failed" apart from "empty" (`discovery_tests::failed_retrieval_is_recorded_apart_from_an_empty_run`).
- **Download source:** verified candidates wait visibly (`verified_candidates_wait_visibly_when_no_download_source_is_set_up`).
- **Metadata lookup:**
  - rejected keys pause the queue until a key is saved;
  - outages reschedule without using up attempts (`metadata_lookup_tests`).
- **Catalogue:**
  - outages wait, refusals stop, and sign-in problems pause;
  - lookups return nothing when signed out (`sharing::tests::outages_wait_rejections_stop_and_acks_are_recorded`).
- **Keychain:** when unavailable, the app reports it and keeps running (`credentials::keychain_unavailable`).

## Clean-machine checks

Run on a fresh user account (or VM) for each platform, using the draft's installers. Record results in the validation report.

1. Install from the installer; the app opens and shows onboarding.
2. Choose a music folder with at least 20 tracks of mixed formats (FLAC, WAV, AIFF, MP3, AAC); import finishes and playback works.
3. Save a Discogs token in Settings; it lands in the OS credential store (Keychain Access, Windows Credential Manager, or the Secret Service on Linux), not in the data folder. Restart: the connection still works without re-entering it.
4. Close the window: the app keeps running in the tray, and background work continues. Quit from the tray: it stops.
5. Force-quit during an import and during analysis; reopen: work resumes with nothing duplicated.
6. Disconnect the network; the library, review, playback, playlists and export still work, and the Activity view explains what is waiting.
7. Export a playlist as Rekordbox XML, import it into Rekordbox, and record the Rekordbox version. Check order and that every track plays.
8. Uninstall; the data folder stays unless removed by hand (documented in the README).

## Validation report

Publish with each release, in `docs/releases/vX.Y.Z.md`:
- **App:** version, commit, and whether each installer was signed.
- **Operating systems tested:** macOS version on Apple Silicon and on Intel, Windows 11 build, and Ubuntu LTS version. Other Linux distributions are best effort and listed separately.
- **Reference hardware:** CPU, memory, and disk type.
- **Versions:** analysis model (id and checksum); slskd version (pinned in `crates/connectors/src/slskd/install.rs`); local model provider and model used for page reading; Rekordbox version.
- **Supported audio formats**, as tested in step 2.
- **Results** of each clean-machine check, with any failures and their issue numbers.

## Licence

Still to be chosen by the maintainer. The inputs are in `docs/decisions/0002-dependencies.md`:
- every linked dependency allows any licence choice (symphonia's MPL-2.0 applies only to its own files);
- slskd (AGPL-3.0) runs as a separate, unmodified program;
- the Essentia models (CC BY-NC-SA 4.0) are downloaded by the user and allow use in a free app only;
- the rusty-chromaprint coefficient question needs an answer from its author before release.

Add `LICENSE` and the `license` field in `Cargo.toml` and `app/package.json` once chosen.
