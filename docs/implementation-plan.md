# Crate Digger — Implementation Plan

This plan implements the [product specification](product-spec.md). The repository currently contains documents only. Milestones below describe future application work.

## 1. Local foundation

Build the Rust core, Tauri/TypeScript shell and SQLite persistence. Add library import in place, file availability, playback, ratings, playlists and durable jobs. Use mocked discovery until integrations exist.

Define stable domain IDs, migrations and user-correction precedence. Store paths on file records, not as track identity. Establish transactional archive promotion and recovery.

Acceptance:

- [ ] Importing never renames or moves existing audio.
- [ ] Library search, playback, rating and playlists work offline.
- [ ] Rating, skip and undo have distinct persisted semantics.
- [ ] Missing and corrupt files retain metadata and produce actionable errors.
- [ ] Restart restores jobs and preferences without duplicate actions.
- [ ] Tray close continues work; explicit Quit stops cleanly.
- [ ] Archive collisions and interrupted moves cannot overwrite or lose audio.

## 2. Audio analysis and identity

Build a separate analysis worker with fingerprint, embedding and explicit-feature outputs. Evaluate a pretrained model on the same recording across formats, sections and masters, alongside different mixes and unrelated tracks. Document the selected weights, checksum, preprocessing, segment policy and runtime before shared data is accepted.

Create identity fixtures that distinguish tracks, versions, releases and files. Benchmark time and memory on the release reference machine. Keep unknown results explicit rather than returning fabricated certainty.

Acceptance:

- [ ] Every feature result carries its analysis version and source identity.
- [ ] Incompatible embeddings cannot enter the same comparison operation.
- [ ] Analysis failure leaves playback and the UI functional.
- [ ] Duplicate copies and conflicting versions follow the identity policy.
- [ ] Temporary audio deletion preserves derived features.
- [ ] Missing audio during a model upgrade is visible and does not cause an endless retry.

## 3. End-to-end discovery

Implement Discogs ingestion and generic public-text ingestion, including pasted text. Add both API and local-model adapters with structured-output validation. Select and document tested provider/model combinations. Preserve source evidence and prevent retrieved content from controlling tools.

Implement YouTube reference lookup and correction, slskd integration, candidate matching and validated acquisition. Calibrate matching against labelled fixtures before allowing automatic downloads. Treat missing YouTube references independently from local audio readiness.

Implement local Tier 2 ranking, cold start, diversity and the 20% exploration allocation. Persist feedback before asynchronous reranking and leave the current track playing. Tune ranking parameters on separate training/evaluation splits and document the resulting configuration.

Implement bounded eager replenishment using the defaults in the product spec. Enforce limits centrally in the job scheduler so independent adapters cannot bypass them.

Acceptance:

- [ ] Both LLM modes yield candidates through the same validated interface.
- [ ] Unsupported candidates cannot automatically enter acquisition.
- [ ] Source text cannot issue shell or filesystem instructions.
- [ ] YouTube links are real references with confidence and provenance.
- [ ] User-corrected metadata and links survive refreshes.
- [ ] Ambiguous mixes require review; failed transfers do not imply dislike.
- [ ] Rating feedback affects both ranking and later source expansion.
- [ ] Reranking meets the specified two-second target on the reference dataset.
- [ ] Queue replenishment stops at resource limits and resumes appropriately.
- [ ] Authentication failures, outages and depleted candidates have distinct visible states.

## 4. Central metadata service

Implement a PostgreSQL-backed service and versioned API schemas for lookup, contributions, feature retrieval, corrections, catalogue cursors and private snapshots. Record deployment and account-authentication choices in an implementation decision before integration; do not ship unauthenticated private-backup endpoints.

Keep shared records and account-owned snapshots separate. Implement durable client outbox processing, idempotent writes, bounded payloads and conflict preservation. Sharing and private backup each require their own setting.

Acceptance:

- [ ] Contract tests cover client/service request and response versions.
- [ ] Retries do not duplicate submissions or discard unacknowledged data.
- [ ] Model/dimension mismatches and oversized submissions are rejected.
- [ ] Shared payloads exclude ratings, credentials and local paths.
- [ ] A client cannot overwrite conflicting contributions silently.
- [ ] An account cannot read, restore or delete another account's backups.
- [ ] Backup restoration preserves playlists and ratings and requests file relinking.
- [ ] Turning off contributions leaves local operation intact.
- [ ] Central outages do not prevent auditioning downloaded tracks.

## 5. Export, packaging and pilot

Implement M3U8 and Rekordbox XML from the documented format. Cover ordered playlists, Unicode, XML escaping, spaces, path encoding, missing files and platform-specific paths. Test real import in a recorded Rekordbox version; schema-valid XML alone is insufficient.

Package and validate macOS Apple Silicon/Intel, Windows x64 and Ubuntu LTS x64. Pin exact OS versions, provider versions and supported codecs in the release report. Select the code licence only after dependency evaluation. Publish hosted-catalogue participation and access terms before operating the service publicly.

Run a consenting DJ pilot comparing cultural-only ranking with combined ranking on held-out feedback. Record keep rate, strong-positive rate, incorrect-version rate, buffer availability and hardware costs. Report results even if the combined model does not outperform the baseline.

Acceptance:

- [ ] M3U8 and Rekordbox imports preserve track order and playable paths.
- [ ] Missing files are reported before export.
- [ ] No invented cues or beat grids are exported.
- [ ] Clean-machine installation, credential storage, tray behaviour and recovery pass on target platforms.
- [ ] Local functionality survives provider and central-service outages.
- [ ] Dependency/model decisions and measured recommendation results are published.

## Test strategy

Use unit tests for identity normalization, rating semantics, archive paths and ranking invariants; integration tests for job recovery, file operations and service contracts; and end-to-end tests for discovery-to-playlist flows. Use fake providers and synthetic audio in routine CI. Keep live integration checks explicit and separate so CI does not depend on network availability or initiate real acquisitions unexpectedly.

Test adversarial page instructions and malformed contribution payloads, not just successful outputs. Exercise crashes at transfer completion, analysis completion, archive promotion and sync acknowledgement boundaries.

## Publication checklist for this planning repository

- [x] Product overview and status in README.
- [x] Detailed product behaviour, defaults and boundaries documented.
- [x] Implementation milestones and acceptance checks documented.
- [x] Local Markdown links and Git whitespace checks pass.
- [x] Initial documents committed and pushed to public `timini/crate-digger`.
- [x] Remote visibility, branch and document availability verified.

No application implementation or code licence is included in this initial publication.
