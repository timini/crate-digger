# Crate Digger

A free, open-source desktop app for DJs to discover, audition, download, organise and playlist music.

**Status: product specification. No application has been implemented yet.**

Crate Digger combines cultural recommendations from tracklists, labels and community sources with audio similarity learned from personal ratings. A local background agent keeps music ready to review. A central service backs up and shares track metadata and audio embeddings; playback, analysis and personal recommendations run locally.

## Documents

- [Product specification](docs/product-spec.md)
- [Implementation plan and acceptance checklist](docs/implementation-plan.md)

## Intended first release

- Rust desktop core for macOS, Windows and Linux, with a Tauri interface.
- Library, discovery and download views.
- API-based and local LLM connections.
- Agent-populated YouTube reference links, local-file import and Soulseek acquisition through slskd.
- Audio analysis, personal ratings and continuously replenished recommendations.
- Playlists with M3U8 and Rekordbox XML export.
- Optional shared metadata and embeddings, plus separate private metadata backup.

## Open-source intention

The intended application will be open source. The code licence will be selected after implementation dependencies are evaluated. This planning repository does not yet grant a software licence, and it contains no application code. Access and licensing terms for any hosted shared catalogue will be specified separately before that service launches.
