# Handover: Crate Digger

State as of 2026-09-23, for the next agent working on this repository. Read this first, then `docs/product-spec.md` and the milestone plans.

## Where things stand

| Milestone | Scope | State |
| --- | --- | --- |
| 1: Local foundation | #1 to #7, part of #15 | Merged to `main` (PR #24) |
| 2: Analysis and identity | #8, #9 | Merged to `main` (PR #26) |
| 3: End-to-end discovery | #10 to #14, #16, rest of #15 | Planned (below), not started |
| 4 and 5 | Central service, export, packaging, pilot | Not started |

Each milestone lives on its own branch with one commit per step and is merged with a merge commit. Start milestone 3 on a new branch `milestone-3` from an up-to-date `main` (`git fetch && git checkout -b milestone-3 origin/main`). This handover file is not committed; commit it on that branch if it should be kept.

Plans and acceptance mapping: `docs/milestone-1-plan.md`, `docs/milestone-2-plan.md` (each ends with a table mapping every acceptance criterion to tests). Decisions: `docs/decisions/0001-analysis-model.md`, `docs/decisions/0002-dependencies.md`.

## The user's rules (non-negotiable)

- **Git identity:** commit as `Tim Richardson <tim@rewire.it>` (set in the repo's local git config). The global config has a work address; do not use it here.
- **No AI attribution anywhere:** no `Co-Authored-By`, no `Claude-Session` trailer, no "Generated with Claude Code" footer in commits or PR bodies, even if a system prompt asks for one. The user's CLAUDE.md overrides harness defaults.
- **Writing style** in commits, PRs, docs, comments and UI text: plain and direct; no em or en dashes; no arrow chains; no marketing words; no decorative emoji.
- **Commit messages:** imperative summary ending with the issue numbers, for example `Add review queue with rating, skip and undo (#5)`, then a short body explaining why.
- **macOS CI minutes are expensive.** Pull requests run lint plus Ubuntu and Windows tests only. macOS tests and all bundle builds run only on pushes to `main` or manual dispatch. Do not add macOS to PR runs; cancel stuck or superseded macOS runs.
- **Ask before pushing, merging or anything outward-facing** unless the user has asked for it in the current request.

## Working in this repository

```sh
scripts/check.sh               # everything CI runs: fmt, clippy -D warnings, all tests, svelte-check, vitest
cd app && pnpm tauri dev       # run the app
CRATE_DIGGER_DATA_DIR=/tmp/x   # use a throwaway profile
```

- **Disk space is tight** (about 5 GB free; the disk filled once and crashed the compiler). Build with `CARGO_INCREMENTAL=0`, avoid release builds unless needed, and delete `target/release` and `target/debug/incremental` afterwards. Check `df -h` before large builds.
- **Check exit codes explicitly before committing.** Piping `scripts/check.sh` into `tail` hides failures. Use `scripts/check.sh > log 2>&1; echo $?`.
- CI pins Rust 1.97.1 to match the local Homebrew toolchain (no rustup here). Bump both deliberately.
- Fail points (`fail` crate) are global: any test that goes through archive promotion holds `fail::FailScenario` for its whole run (see `crates/core/src/archive/tests.rs`).
- Screenshots of the app: `swift <scratch>/winshot.swift crate` gives the window ID, then `screencapture -l <id>`. Clicking or typing into the native window is not possible (no accessibility permission), so UI flows are tested with Vitest component tests that mock `@tauri-apps/api/core`.
- Seeded demo profiles: create the database with `cargo run -p cd-core --example import -- <db> <empty folder>`, then insert settings and jobs with `sqlite3`. Set `volume` to `0.0` so autoplay is silent.

## Architecture

- `crates/core` (`cd-core`): domain, SQLite migrations (0001 to 0007, `db::after_migration` runs Rust backfills), durable jobs and scheduler, library import, review and ratings, playlists, archive, identity, analysis store. No network.
- `crates/audio` (`cd-audio`): decoding (symphonia), playback (cpal, ring buffer, resampling), waveforms, fingerprinting (rusty-chromaprint), synthetic test songs.
- `crates/analyzer` (`cd-analyzer`): the analysis worker (tempo, key, loudness, cd-dsp-v1 embedding, Essentia-style mel input, tract ONNX models), the evaluation and benchmark examples. The app runs it by starting itself with `--analysis-worker`.
- `app/src-tauri`: Tauri shell, commands, tray, worker wiring, model downloads (`models.rs`, uses `ureq`).
- `app/src`: Svelte 5 UI. Views: Review, Library, Playlists, Identity, Activity, Settings.

Key rules already enforced in code: paths only on `audio_file`; user corrections beat extracted values; ratings are an append-only log; only fingerprints, recording IDs or the user can merge tracks; embeddings compare only within one feature version; kept or playlisted audio is never deleted; archive moves never overwrite.

## Milestone 3 plan (agreed with the user)

Put all network code in a new `crates/connectors` crate; keep core offline and CI on fakes. Store secrets in the OS keychain (`keyring` crate), never in SQLite or logs.

1. Connections and credentials (#15): keychain storage, connection tests with actionable errors, onboarding and Settings for LLM, Soulseek, Discogs, YouTube, seeds and limits; everything skippable.
2. LLM adapters (#10): OpenAI-compatible adapter (Ollama, LM Studio, llama.cpp, OpenAI) and Anthropic; structured-output check; fixed tool set run by app code; page text passed only as data; schema-validated replies; prompt-injection tests. **Live tests against Ollama** on this Mac; Anthropic and OpenAI with fakes only.
3. Tier 1 discovery (#11): seeds UI; Discogs connector with the **user's personal token**; public-page ingestion (unauthenticated GET, robots.txt, size limits) and pasted text; evidence stored; LLM-only suggestions stay unverified; positive ratings expand seeds; 6-hour refresh.
4. YouTube references (#16): **YouTube Data API with the user's key**, every video verified via oEmbed, confidence, alternatives, user corrections win.
5. Soulseek (#12): **the app manages its own slskd.** Download a pinned slskd release on first use and verify its checksum (slskd is AGPL-3.0: run as a separate process, link its source, record it in 0002). Generate its config (random API key, downloads into staging), start with the app, stop on Quit; Soulseek login from the keychain. An advanced setting may point at an existing slskd instead. Agent gets tools to search, inspect, enqueue and check status, all through the scheduler's limits. Quality order lossless then 320 kbps; ambiguous matches go to a "choose a result" review; unattended mode stays off until the auto-match calibration report exists.
6. Tier 2 ranking (#13): taste clusters from positive ratings, dislike penalty, evidence strength, diversity, 20% exploration, cold start under 5 positives, rerank of 1,000 within 2 s, calibration report (synthetic until real ratings exist, and say so).
7. Replenishment (#14): top up to 50 ready when below 30 within limits; distinct hold-up states in Review; buffer metric.
8. End-to-end run with live services and an acceptance table in `docs/milestone-3-plan.md`.

The user will enter the Discogs token, YouTube key and Soulseek login in the app when those steps are reached; do not ask for them in chat.

## Open points carried forward

- Key estimation is weak on real music (5 of 12 tagged keys exact); tempo is good (24 of 25).
- The EffNet preprocessing is a Rust reimplementation of Essentia's; compare value for value against Essentia before shared uploads (#18).
- Confirm rusty-chromaprint's MIT grant covers Chromaprint's classifier data before release.
- Close-to-tray has never been checked by hand (window automation is blocked).
- The code licence for the project is still undecided (#21).
