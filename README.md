# Crate Digger

A free, open-source desktop app for DJs to discover, audition, download, organise and playlist music.

**Status: milestone 1 (local foundation) implemented: library import, playback, review queue with ratings, playlists and archive. Discovery sources arrive in milestone 3; a demo mode generates tones to try the review flow.**

Crate Digger combines cultural recommendations from tracklists, labels and community sources with audio similarity learned from personal ratings. A local background agent keeps music ready to review. A central service backs up and shares track metadata and audio embeddings; playback, analysis and personal recommendations run locally.

## Documents

- [Product specification](docs/product-spec.md)
- [Implementation plan and acceptance checklist](docs/implementation-plan.md)
- [Milestone 1 plan](docs/milestone-1-plan.md)

## Development

Requirements: Rust (stable), Node 24 and pnpm 11. On Linux, also the [Tauri system packages](https://v2.tauri.app/start/prerequisites/) and `libasound2-dev`.

```sh
cd app
pnpm install
pnpm tauri dev          # run the app
pnpm test               # frontend tests
cargo test --workspace  # Rust tests (from the repo root)
scripts/check.sh        # everything CI checks, from the repo root
```

To index a folder from the command line (useful for trying a real library):

```sh
cargo run --release -p cd-core --example import -- <database.sqlite> <music folder>
```

Layout:

- `crates/core`: domain model, SQLite schema and migrations, jobs, library, archive and adapter interfaces. No UI and no network access.
- `crates/audio`: decoding, playback and waveforms.
- `app/src-tauri`: the Tauri shell that wires the core to the UI.
- `app/src`: the Svelte and TypeScript interface.

Set `CRATE_DIGGER_DATA_DIR` to use a data directory other than the platform default.

## Intended first release

- Rust desktop core for macOS, Windows and Linux, with a Tauri interface.
- Library, discovery and download views.
- API-based and local LLM connections.
- Agent-populated YouTube reference links, local-file import and Soulseek acquisition through slskd.
- Audio analysis, personal ratings and continuously replenished recommendations.
- Playlists with M3U8 and Rekordbox XML export.
- Optional shared metadata and embeddings, plus separate private metadata backup.

## Open-source intention

The intended application will be open source. The code licence will be selected after implementation dependencies are evaluated. This repository does not yet grant a software licence. Access and licensing terms for any hosted shared catalogue will be specified separately before that service launches.
