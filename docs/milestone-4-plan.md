# Milestone 4: playlist export, then the central service

Playlist export (#20) comes first because it gets tracks into DJ software now. The central service follows: #17 hosted API, #18 sharing and sync, #19 private backup. Hosting is Cloud Run with Firestore and a Cloud Storage bucket for backups; sign-in is with Google. Firestore was chosen over Bigtable because Bigtable bills per node-hour even when idle, while Firestore bills per operation, has a free tier and supports vector search for embeddings.

## Steps

1. Playlist export: check first, then M3U8 and Rekordbox XML (#20).
2. Decision record for the central service (`docs/decisions/0003-central-service.md`).
3. Shared protocol crate with versioned types and contract snapshots.
4. The service: Google ID token auth, in-memory and Firestore stores, contributions, features, corrections, catalogue changes, backups, validation and rate limits.
5. Desktop sign-in with Google (loopback OAuth with PKCE, refresh token in the keychain).
6. Sharing client (#18): outbox, payload builder, feature reuse.
7. Private backup and restore (#19).
8. Deployment script, run only when the user provides a project and approves the cost.

## Acceptance evidence

| Area | Required evidence | Status |
| --- | --- | --- |
| Export | Order and playable paths kept, missing files reported first, no invented cues or beat grids, Rekordbox version recorded | Done except the Rekordbox import, which needs the user's Rekordbox (not installed on this Mac). `cd-core` `export::tests`: Rekordbox-style file URIs (spaces, `&`, `#`, `%`, accents, Windows drive letters); M3U8 order and single-line entries; the XML parses back with collection and playlist order, repeats and escaping intact, and has no `POSITION_MARK` or `TEMPO` elements; missing and temporary files are reported, and tempo and key estimated by analysis are left out while tags and user edits are kept. `ExportPlaylist.test.ts`: problems shown before anything is written. |
| Central service | Contract tests, validation, no silent overwrites, account isolation | Pending |
| Sharing | No duplicate contributions, payload exclusions, compatible feature reuse, local use unaffected | Pending |
| Backup | Restore keeps playlists and ratings and asks for relinking, owner-only access, idempotent upload | Pending |
