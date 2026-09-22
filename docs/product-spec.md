# Crate Digger — Product Specification v0.1

Status: approved planning baseline; application not yet implemented.

## 1. Product definition

Crate Digger is a free, open-source desktop app that helps DJs discover, audition, download, organise and playlist music. Its core promise is to keep a fresh, relevant queue of tracks ready to hear.

The app runs locally on macOS, Windows and Linux. A background agent finds culturally relevant tracks, obtains audio for analysis, and learns from the user's ratings. Downloading serves both analysis and auditioning: a track need not enter the permanent library for its embedding to remain useful.

A central service backs up and shares track metadata and audio embeddings. Personal recommendation processing stays local. The central service is not required for library access, playback or reviewing downloaded tracks. V1 is a useful crate-digging application, not a distributed data-harvesting or community-model training platform.

### First-release outcomes

A user can:

- Import music and choose artists, labels, DJs or tracks as discovery seeds.
- Configure an API-based LLM or a local model.
- Receive culturally relevant candidates with traceable source evidence.
- Obtain matching YouTube reference links automatically.
- Download through Soulseek and analyse imported or downloaded audio.
- Rate tracks with thumbs down or one to three stars.
- See recommendations adapt as they rate.
- Organise retained music and create playlists.
- Export playlists to M3U8 and Rekordbox XML.
- Optionally share metadata and embeddings, and separately back up private preferences.

### Outside v1

Mobile apps, cloud audio storage, community rating feeds, shared taste-model training, custom audio-model training, YouTube audio extraction or playback capture, authenticated scraping, arbitrary website interaction, direct Rekordbox database management and an always-on operating-system service.

## 2. User experience

### Onboarding

Setup configures existing music folders, a managed archive location, an agent connection, a connected slskd instance, discovery seeds, resource limits and optional central-service participation.

Every integration can be skipped and completed later. Connection tests explain missing configuration. Only dependent capabilities are disabled. Import indexes existing files in place and never moves or renames them automatically.

### Library

The library shows indexed tracks, file availability, metadata, ratings, analysis status and playlist membership. Search and filters cover artist, title, mix, label, rating, tempo, estimated key and availability.

Users can edit metadata, resolve duplicates, relink missing files and open containing folders. User corrections are retained separately from extracted values and take precedence over later automatic updates.

Playback includes play/pause, seeking, volume, elapsed time and a waveform when available. Playback takes priority over background analysis. Missing or corrupt files produce actionable errors without removing metadata or ratings.

### Discovery

One track is presented at a time with artist, title, exact mix/version where known, available artwork, playback controls, YouTube reference links, recommendation reasons, source evidence and match confidence.

Actions are rating, skip, undo, keep and add to playlist. Buttons and keyboard shortcuts are primary desktop controls; swipe gestures supplement them.

| Action | Meaning |
| --- | --- |
| Thumbs down | Negative taste signal; suppress this version from future automatic review |
| One star | Some interest; weak positive signal |
| Two stars | Strong interest |
| Three stars | Favourite; strongest positive signal |
| Skip | No explicit preference; defer for this session |
| Undo | Reverse the last rating or skip |

Ratings, archival decisions and playlist membership are independent. A rating does not automatically retain audio. Tracks without playable local audio remain pending and do not count toward the ready-to-review buffer. A YouTube link alone does not make a track ready.

### Downloads and background activity

Show searches, proposed matches, transfers, validation, analysis and failures. Users can pause all work, cancel jobs, retry failures and correct matches.

Closing the window into the tray keeps workers active. Explicit Quit records resumable state and stops workers. V1 does not install a separate always-on service.

| Setting | Configurable default |
| --- | --- |
| Ready-to-review target | 50 tracks |
| Replenishment threshold | Fewer than 30 ready tracks |
| Active downloads | 2 |
| Concurrent analysis jobs | 1 |
| Temporary audio budget | 10 GB |
| New acquisition jobs | At most 100 per day |
| Source refresh | Every 6 hours while running |

Limits pause affected work and expose the reason. Temporary acquisition stops when staging fills; unreviewed audio is not silently deleted to free space. Permanent files are never deleted to satisfy a budget. Users can explicitly clear temporary files while retaining metadata and features.

### Archive

Downloaded audio enters staging before validation. Keep promotes validated audio into the archive. Imported audio remains in its original location unless the user explicitly requests management.

Default template: `Artist/Release/Track number - Title (Mix).extension`.

Use `Singles` for missing releases; omit missing track numbers and empty mix suffixes. Sanitize filenames across supported platforms. Collisions never overwrite files. Promotion updates references transactionally, and interrupted promotion must recover without data loss. Tracks referenced by playlists are retained.

### Playlists and export

Support create, rename, reorder and delete. Removing a playlist does not delete audio.

Export M3U8 with local paths and Rekordbox XML with supported metadata and ordered playlist membership. Validate file availability before export and report missing tracks explicitly. Do not export invented cue points or inferred beat grids.

Rekordbox support uses the [documented XML format](https://rekordbox.com/en/support/developer/). Record the exact Rekordbox version used for import testing; do not assume historical UI instructions apply to every version. Export generation is supported on all target platforms, while native Rekordbox import is tested where Rekordbox runs.

## 3. Cultural discovery and track identity

### Tier 1: candidate generation

Expand seeds and positively rated tracks through Discogs artist/label/release relationships, supplied list and tracklist URLs, public DJ tracklists, and forum or editorial recommendation pages.

V1 has a dedicated Discogs connector and a general public-text-page ingestion connector. Users can paste text when retrieval fails. General ingestion does not promise support for every website.

Each candidate retains its source URL or supplied-text reference, retrieval time, supporting evidence and extraction confidence. Unsupported LLM suggestions stay unverified and never automatically enter acquisition.

The agent uses structured tool calls and validated outputs. Pages and supplied text are data, never instructions. The agent cannot execute arbitrary commands or modify unrelated files.

### Canonical identity

Distinguish recordings/mixes, releases containing them and individual audio files. Maintain aliases and external identifiers without conflating title similarity with identity. Original mixes, remixes, edits, pitched copies and recordings from sets require explicit matching evidence.

Fingerprints identify audio; embeddings measure similarity. Neither an LLM assertion nor embedding proximity alone may merge canonical records. Conflicts retain provenance and enter review.

### YouTube metadata

The agent searches by artist, title and mix. Store video ID, URL, title, channel, stated duration when available, lookup time and match confidence. Retain alternatives and a preferred link. User-corrected links take precedence.

Unavailable or uncertain matches remain unresolved. Never invent links. Missing YouTube metadata does not block a confidently identified local file from analysis or review. YouTube links are references, not acquisition inputs, in v1.

## 4. Acquisition, analysis and recommendations

### Acquisition

Audio comes from existing local files or Soulseek through a connected slskd service. Integrate with slskd rather than implementing the Soulseek protocol. The acquisition adapter exposes search, inspect results, enqueue, progress, cancel and retry. Connection settings and credentials remain local. See [slskd documentation](https://github.com/slskd/slskd).

Match on artist, title, mix, duration, format and available quality metadata. Prefer supported lossless files, then 320 kbps MP3. Other qualities require user selection. Codec decoding support must be documented and tested before a format is advertised.

Automatic acquisition requires an unambiguous match. Conflicting mixes or materially inconsistent durations require review. Calibrate the auto-match rule against the version-matching fixture set before enabling unattended downloads.

Validate decodability, duration and identity before admission to the ready queue. Failed downloads or bad matches are operational failures, not dislike signals.

### Analysis and features

Extract a fingerprint, versioned music embedding, duration, estimated tempo, estimated key, loudness, segment coverage and extraction-quality information.

Use a pretrained music model. The first analysis milestone compares consistency across copies and sections of the same recording. Select and pin model weights and preprocessing before accepting shared contributions. A candidate family is documented in [Essentia's model catalogue](https://essentia.upf.edu/models.html); this is a research starting point, not an approved dependency or licence selection.

Every feature record includes model identifier, weights checksum, preprocessing version, source fingerprint and analysed segment boundaries. Keep incompatible versions separate; never compare their vectors directly.

Check the shared catalogue before extracting a missing embedding. Reuse only when recording identity and analysis version match confidently. Local availability and validation are still required for auditioning. If uncertain, analyse locally.

Retain features after temporary audio deletion. Model upgrades cannot assume deleted source audio remains available: mark old features by version and schedule new analysis only when suitable audio is available.

### Tier 2: personalised ranking

Combine similarity to several positively rated tracks, a negative adjustment for disliked-track similarity, Tier 1 evidence strength and diversity controls. Preserve multiple taste clusters rather than reducing taste to one average vector.

Reserve 20% of review positions for culturally relevant exploration. Before five positive ratings exist, prioritise cultural evidence and seed similarity. Exact dislikes suppress that version; they do not globally blacklist an artist or label.

Persist ratings immediately and rerank asynchronously. Never interrupt the playing track. Positive ratings feed source expansion. Undo restores the previous effective preference and triggers reranking.

Detailed ranking weights are calibrated in the recommendation milestone using held-out ratings; report the chosen configuration and comparison results. This specification fixes intended behaviour and evaluation, not unvalidated model-quality claims.

### Durable pipeline

`Candidate → Identified → Acquisition queued → Downloading → Validating → Analysing → Ready → Reviewed`

Existing local files can bypass acquisition; compatible shared features can bypass embedding extraction. Required local validation still runs.

Paused, blocked, failed and cancelled states carry reasons. Persist transitions and resume after restart without duplicate transfers or contributions. Use bounded network retries with backoff. Authentication failures pause the connector until corrected. Distinguish unavailable sources, exhausted budgets and insufficient candidates in the UI.

## 5. Architecture and central database

### Local architecture

- Rust core and Tauri shell with a TypeScript interface.
- SQLite for catalogue, jobs, ratings, playlists and sync outbox.
- Separate audio-analysis worker so inference failures do not terminate the UI.
- Adapters for sources, LLM providers, acquisition and central sync.
- Operating-system credential storage for secrets.

Support local and API-based models at launch. Configure endpoint, model and credentials as needed; validate structured-output capability before enabling automated discovery. Show request limits and provider usage when available. A provider failure cannot prevent playback or rating.

Target macOS Apple Silicon and Intel, Windows x64 and Ubuntu LTS x64. Other Linux distributions are best-effort until tested. Pin supported OS versions and reference hardware in the release validation report.

### Domain records

Track; release association; local audio file; external source/evidence; YouTube match; versioned feature record; candidate and explanation; rating event; playlist and ordered membership; durable job; contribution; backup operation.

Stable IDs provide identity independently of paths. Preserve provenance and user corrections. Separate transfer state from preference state.

### Minimum central interfaces

- Batch lookup by external identifiers and fingerprints.
- Submit metadata and feature contributions.
- Retrieve compatible feature records.
- Submit corrections while retaining conflicting evidence.
- Upload, list and restore private backup snapshots.
- Retrieve catalogue changes through a cursor.
- Delete a user's private backups.

Writes carry idempotency identifiers. Shared records are separate from account-owned backup records. Define and version request/response schemas before client/service integration; implement contract tests for those schemas.

### Central responsibilities

Use a PostgreSQL-backed API service for metadata, embeddings, provenance and private snapshots. V1 does not run discovery, acquire or host music, train shared taste models or distribute analysis tasks to users.

Shared contributions contain metadata, references, fingerprints and embeddings. Exclude local paths, credentials and ratings. Contribution participation is explicit and independently switchable; local features continue when sharing is off.

Private backup is separately enabled and includes ratings, seeds, playlists and library metadata. Restore requires relinking local audio. V1 uses explicit snapshots, not live multi-device conflict resolution. Explain that music files and credentials are not backed up.

Treat incoming contributions as untrusted. Validate schemas, vector dimensions, model versions and payload limits; rate-limit accounts and retain conflicts for reconciliation. One client cannot silently overwrite others' evidence.

Central outages queue uploads in a durable outbox without blocking local use. Do not discard unacknowledged contributions. Backups are visible only to their owner. Public-catalogue access and retention terms must be stated before service launch.

## 6. Acceptance and evaluation

### Functional acceptance

- Import, playback, rating and playlist creation work without the central service.
- API and local models produce validated candidates through the same pipeline.
- Candidate evidence and YouTube match status are inspectable.
- Original/remix ambiguity, duplicate files and incorrect matches are handled explicitly.
- Restart resumes transfer and analysis without duplicate work.
- Replenishment respects configured storage, concurrency and daily limits.
- Ratings persist immediately; reranking completes within two seconds for a 1,000-track ready/candidate set on documented reference hardware.
- Repeated sync retries create no duplicate contributions.
- Feature reuse requires confident identity and compatible analysis versions.
- Shared metadata never includes private ratings or local paths.
- Private backups restore preferences and playlists with explicit audio relinking.
- M3U8 and Rekordbox XML preserve order and resolve supported paths.
- No silent overwrites or deletion of retained files occur.

### Evaluation

Compare Tier 1 alone against combined ranking using held-out ratings from consenting pilot DJs. Measure keep rate, strong-positive rate, incorrect-version rate and ready-buffer availability. Avoid evaluating on the same rating events used to fit preferences.

Also measure extraction time and peak memory, embedding stability across copies, source retrieval failures and queue recovery. Report results and limitations; recommendation improvement is a hypothesis until measured.

### Release gates

Passing local core and recovery checks; validated matching before unattended acquisition; pinned analysis versions before shared uploads; account isolation before private backup; tested exports and recorded target-platform compatibility before release.

## 7. Delivery and open decisions

Deliver local foundation, analysis/identity, end-to-end discovery, central service, then release validation. The companion [implementation plan](implementation-plan.md) defines milestones and acceptance checks.

This is a product baseline rather than a claim that all implementation choices have been validated. Model selection, calibrated matching/ranking parameters, specific provider compatibility, deployment/authentication implementation, supported OS versions and the code licence are milestone decisions. Resolve and record them before their corresponding release gates; do not silently turn proposed dependencies into commitments.

The intended app is open source. The initial repository contains planning documents only. Select a code licence after dependency evaluation, and state hosted-catalogue terms separately before launch.
