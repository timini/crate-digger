# Milestone 1: local foundation (Crate Digger)

## Context

`timini/crate-digger` (cloned to `~/Documents/projects/personal/music/crate-digger`) holds only a product spec, an implementation plan and 23 GitHub issues. Milestone 1 is epics #1 to #7 plus the folders/archive part of #15. The outcome is an offline app where a DJ can import music, play it, review mocked candidates, rate them, build playlists and promote downloads into an archive, with no network. The repo has no code yet, so everything below is new. Disk now has 14 GB free, which is enough.

## Decisions (confirmed or chosen)

- **Playback in Rust** (confirmed by user): `symphonia` decode + `cpal` output on a dedicated thread.
- **UI framework: Svelte 5 + TypeScript + Vite** (chosen for the user). Reasons: it is the leanest of the options (less boilerplate than React), its fine-grained updates suit a playback clock and waveform that change many times a second, it has an official Tauri template, and its ecosystem is much larger than Solid's. Plain Vite SPA, no SvelteKit. Tests with Vitest and `@testing-library/svelte`.
- Tauri 2, `rusqlite` (bundled), UUIDv7 IDs, `lofty` for read-only tag reading, `blake3` content hashes, SQLite FTS5 for search, `tracing` for logs, `fail` crate for crash injection.
- Branch `milestone-1`, one commit per step, message style `E0X <description>` to reference the epic (repo has no established style beyond plain sentences; confirm from `git log` at first commit). No AI attribution. No push until the user asks.

## Layout

```
Cargo.toml                 workspace
crates/core/               domain, db, migrations, jobs, library, ratings, playlists, archive, adapters
  migrations/0001_init.sql
  src/{domain,db,jobs,library,review,playlists,archive,adapters}/
crates/audio/              decode, player thread, waveform peaks, format registry
  tests/fixtures/          tiny synthetic sine tones (wav, aiff, flac, mp3, m4a, ogg)
app/src-tauri/             Tauri shell: commands, tray, lifecycle, worker startup
app/src/                   Svelte UI: Library, Review, Playlists, Activity, Onboarding, Settings
scripts/gen-fixtures.sh    ffmpeg script that generated the committed fixtures
.github/workflows/ci.yml
docs/milestone-1-plan.md   copy of this plan
docs/formats.md            advertised codecs and their decode tests
```

## Steps (one commit each)

1. **Foundation (#1)**
   - Workspace, Tauri + Svelte scaffold, `0001_init.sql` covering all spec domain records: `track`, `release`, `track_release`, `audio_file` (only place a path lives), `field_value` + `field_correction` (effective view: correction wins), `candidate` + `evidence`, `youtube_match`, `feature_record`, `rating_event`, `keep_decision`, `playlist` + `playlist_entry`, `job`, `contribution`, `backup_operation`, `sync_outbox`, `archive_op`.
   - Migrator with `schema_migrations`, WAL, foreign keys on.
   - Adapter traits `DiscoverySource`, `LlmProvider`, `Acquirer`, `CentralSync` with fakes.
   - CI matrix (macOS, Windows, Ubuntu): `cargo fmt --check`, `clippy -D warnings`, `cargo fetch` then `cargo test --offline` (Linux inside `unshare -n`), `pnpm lint/check/test`, unsigned `tauri build`.
   - Tests: migrate empty DB; migrate a DB at an older version; correction survives re-import; no path column on `track`.
2. **Durable jobs (#2)**
   - `job` rows: kind, state (`queued|running|paused|blocked|failed|cancelled|done`), reason (required for non-running states, DB CHECK), attempts, `next_run_at`, lease owner/expiry, unique `idempotency_key`.
   - Candidate pipeline state enum with the spec transitions and bypass paths, validated in one `transition()` function.
   - `Scheduler` owns per-kind concurrency, daily counters and storage budget; workers only get work via `scheduler.claim()`.
   - Backoff with cap; `ConnectorAuthFailed` pauses every job of that connector with a reason.
   - Startup recovery requeues expired leases.
   - Crash tests with `fail` points at transfer completion, analysis completion, archive promotion and sync ack: panic, reopen DB, recover, assert no duplicate effects.
   - Pause all / cancel / retry commands.
3. **Library import (#3)**
   - Read-only walk (`walkdir`), `lofty` tags, blake3 hash of the file; test snapshots path/mtime/bytes before and after import.
   - FTS5 search + filters (artist, title, mix, label, rating, tempo, key, availability).
   - Metadata edit writes `field_correction`.
   - Duplicate groups by hash with primary selection.
   - Relink by hash, then size + duration.
   - Availability check marks missing/corrupt with actionable messages; nothing is deleted.
   - "Open containing folder" via `tauri-plugin-opener`.
   - 10k-track generated fixture DB + search benchmark test (target: under 50 ms per query).
4. **Playback (#4)**
   - Player thread: play/pause/seek/volume/position events to the UI; waveform peaks computed once and cached in the DB.
   - Global "playback active" flag that the scheduler reads so analysis-type jobs yield.
   - Decode test per advertised format using committed fixtures; `docs/formats.md` lists only formats with passing tests.
   - Missing/corrupt file returns a typed error shown in the UI.
   - Underrun-counting harness run under synthetic CPU load.
5. **Playlists (#6)**
   - Create, rename, reorder (integer positions rewritten in one transaction), delete (membership only).
   - Add from library and review card.
   - Playlist-referenced files protected from temp cleanup.
6. **Review queue (#5)**
   - `FakeSource` produces candidates with evidence, backed by fixtures.
   - Ready query requires an available, playable `audio_file`; test proves YouTube-only candidates never appear.
   - Actions: thumbs down, 1 to 3 stars, skip (session-scoped), undo (reverts last event), keep, add to playlist.
   - Keys: `0`, `1`/`2`/`3`, `S`, `Z`, `K`, `P`, `Space`, arrows to seek; swipe supplements.
   - Rating commits, then fires an async rerank hook (no-op until #13); never touches the player.
   - Unit tests for each semantic, plus "rating does not create a keep decision".
7. **Staging and archive (#7)**
   - Template `Artist/Release/NN - Title (Mix).ext` with `Singles` and omission rules.
   - Sanitiser tests for Windows reserved names, invalid characters, trailing dots/spaces, 255-byte limit, NFC.
   - No-replace move (`renamex_np RENAME_EXCL`, `renameat2 RENAME_NOREPLACE`, `MoveFileExW` without replace) with copy+fsync+verify+delete fallback; collisions get ` (2)`.
   - `archive_op` journal (`intent -> copied -> db_updated -> source_removed`) and startup recovery; crash test between every step.
   - Staging budget pauses acquisition with a reason; "Clear temporary files" keeps metadata and features and skips playlist-referenced or kept files.
8. **Shell wiring and onboarding (#2 tray, #15 partial)**
   - Tray: window close hides and workers continue; Quit stops claims, parks running jobs as `paused` ("app quit"), exits.
   - First-run: music folders and archive location; other integrations shown as "Set up later".
   - Settings screen for the same.
   - End-to-end pass.

## Verification

- `cargo test --workspace --offline` and `pnpm test` pass locally; `cargo clippy -D warnings` and `pnpm check` clean.
- `pnpm tauri dev` on this Mac: import a folder of real files and confirm they are untouched (`find -newer` / hashes), play and seek, rate/skip/undo from the keyboard only, build and reorder a playlist, restart and confirm state, close to tray while fake jobs run, Quit and relaunch to confirm jobs resume without duplicates.
- Crash-injection and archive recovery tests pass.
- CI green on all three OSes once the user allows a push; that closes "launches on all three OSes".
- Map each acceptance checkbox in issues #1 to #7 to a named test in a short table in `docs/milestone-1-plan.md`.

## Known limits

- Windows and Linux launch is proven by CI builds, not by me running them.
- Playback smoothness is measured on this Mac; reference hardware is chosen in #21.
