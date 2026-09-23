# Milestone 2 plan: analysis and identity

## Context

Milestone 1 is on branch `milestone-1`, open as PR #24. Milestone 2 covers #8 (canonical track identity and version matching) and #9 (audio analysis worker and model selection). These gate later work: unattended acquisition (#12), personal ranking (#13) and feature sharing (#18) all need to know exactly which recording a file is and need versioned embeddings.

Decisions already made by the user:
- **Model licence:** non-commercial weights (for example Essentia's CC BY-NC-SA models) are acceptable because the app is free. Pick whatever evaluates best; the licence is recorded in the decision document.
- **Evaluation corpus:** the user's own library, run locally. Only aggregate metrics are committed; no audio, paths or track names.

Work continues on a new branch `milestone-2` from `milestone-1`. Commits use `tim@rewire.it` and carry no AI attribution (the user's CLAUDE.md overrides any harness default). No push without asking.

## What exists to build on

- `crates/core/migrations/0001_init.sql`: `feature_record` (model_id, weights_checksum, preprocessing_version, source_fingerprint, segment bounds, embedding BLOB, tempo, key, loudness, quality) with `audio_file_id ON DELETE SET NULL`, so features already outlive temporary audio. Also `track_redirect`, `track_external_id`, `release`, `track_release`.
- `crates/core/src/library/duplicates.rs`: `merge()` (evidence required) and `resolve_track_id()`. Reuse for fingerprint-backed merges.
- `crates/core/src/acquisition.rs`: `AnalyseHandler` computes only a waveform today; it becomes a client of the analysis worker.
- `crates/core/src/discovery.rs`: `identify()` is a placeholder transition; it becomes real matching.
- `crates/core/src/jobs/scheduler.rs`: `ANALYSE` already yields to playback and has its own concurrency limit.
- `crates/audio/src/decode.rs`: the `Decoder` streams f32 audio; `waveform.rs` shows the pattern for whole-file passes.
- `crates/core/test_support/real_probe.rs`, `scripts/gen-fixtures.sh`, fail points, and the `run_one` job test rig.

## Design

### Analysis worker (#9)

- **Separate process.** A new binary crate, `crates/analyzer` (`cd-analyzer`), speaks JSON lines over stdin and stdout: an analyse request goes in, and features, a failure, or a progress heartbeat come out. The app starts it per job with a timeout, a memory limit (`setrlimit` on Unix, a Job Object on Windows) and low priority (nice 10 or below-normal priority). A crash, hang or out-of-memory kill fails the job with a reason and never touches the UI or playback. Tauri ships it as a sidecar (`externalBin`).
- **Features.** Each is stored with its version and source identity:
  - fingerprint: Chromaprint via `rusty-chromaprint`, pure Rust, licence to be checked;
  - integrated loudness in LUFS (`ebur128` crate);
  - estimated tempo (onset-strength autocorrelation);
  - estimated key (chroma with Krumhansl profiles, reported in Camelot and standard notation);
  - duration;
  - segment coverage: three 30 s windows at 25, 50 and 75%, boundaries recorded;
  - quality: fraction decoded, clipping ratio, silence ratio;
  - embedding: per segment, plus their mean.
- **Embedding backends** sit behind a trait with a `FeatureVersion { model_id, weights_checksum, preprocessing_version }`:
  - `cd-dsp-v1`: a deterministic hand-built vector (log-mel statistics, chroma, onset and tempo features). It has no licence or download, is what CI uses, and is the baseline in the evaluation.
  - Pretrained candidates, run with `tract-onnx` (pure Rust, offline build): Essentia Discogs-EffNet and MAEST (CC BY-NC-SA), musicnn, and one CLAP or OpenL3 variant if it converts cleanly. If a model needs operators tract lacks, use `ort` with vendored binaries for that model only and record it.
  - Model weights are not committed. They are fetched once into app data with a pinned SHA-256 and verified before every load.
- **Version guard.** An `Embedding` carries its `FeatureVersion`. `similarity(a, b)` returns `Err(IncompatibleVersions)` unless the versions are identical; there is no raw-vector comparison API. Tests cover the refusal.
- **Model upgrades.** A new `analysis_state` table records, per (track, version), one of: done, queued, needs_audio (with a reason) or failed. When the pinned version changes, a job is queued only where playable audio exists. Everything else is marked needs_audio once, visible in the UI and never retried in a loop.
- **Benchmarks.** The worker reports wall time and peak RSS (`getrusage`, or process memory counters on Windows). A `bench` example records per-minute cost for each backend on this Mac.

### Identity (#8)

- **Normalisation** (`core/src/identity/normalize.rs`):
  - Unicode fold and lower case; strip "feat.", "ft." and "featuring" into a featured-artists list; treat "&", "and" and "x" as equivalent artist separators; strip punctuation.
  - The mix name is classified as original, extended, radio edit, dub, remix (by X), edit, instrumental, live, VIP, or other.
- **Model:**
  - migration `0005_identity.sql` adds `work` (the composition that versions share) and `track.work_id`;
  - `artist_alias`;
  - `identity_evidence (track_a, track_b, kind, detail, source, created_at)`;
  - `identity_conflict` (the review queue, with state and resolution);
  - `audio_file.variant` (NULL or `pitched:+x%`, `section`).
- **Policy** (`identity/policy.rs`): a pure function over evidence, returning `SameRecording | DifferentVersion | Unrelated | Unknown | NeedsReview` plus the evidence used. The rules follow the spec:
  - A fingerprint match plus compatible metadata means the same recording, and an automatic merge is allowed with the evidence recorded.
  - A fingerprint match with conflicting mix metadata needs review.
  - Same work but a different mix class or remixer means a different version. The tracks are linked under one work, never merged.
  - Title similarity alone, embedding proximity alone, or an LLM assertion alone never gives more than Unknown or NeedsReview, and never merges.
  - Pitched copies: a speed-compensated fingerprint match whose duration ratio agrees with the pitch ratio makes the file a pitched variant of the same track. The unpitched copy stays primary.
  - Missing data stays Unknown; certainty is never invented.
- **Labelled fixture set.**
  - A deterministic synthetic song generator (drums, bass, chords and melody from a seed) creates variants at test time:
    - re-encodes: MP3 128 and 320, AAC and FLAC, committed via `scripts/gen-identity-fixtures.sh`, about 2 MB;
    - remasters: EQ and loudness changes;
    - sections, edits (a section cut out), and pitched copies at ±2, 4 and 8%;
    - remixes: the same melody and key with a new drum pattern and tempo;
    - unrelated songs.
  - `fixtures/identity/cases.json` lists pairs with expected verdicts.
  - A calibration test reports per-class precision and recall, and fails if a Different or Unrelated pair is ever called SameRecording.
- **Integration:**
  - Import and acquisition fingerprint files through the analysis job, then run the policy against library tracks with the same work or similar metadata, and against existing fingerprints through an index of fingerprint hash prefixes.
  - A byte-identical copy keeps the milestone 1 behaviour.
  - A re-encode of an owned track auto-merges with evidence.
  - A candidate matching an owned recording is marked "already in library" and skips acquisition.
  - A candidate that is another version of an owned track is kept, with "different version of X" in its reasons.
  - `discovery::identify()` calls the policy.
- **UI:**
  - An Identity review screen lists conflicts showing both sides, the evidence and a play button for each. The actions are: same recording (merge), different version (link), and not related.
  - The track panel gains a Versions section showing other tracks of the same work and file variants.

### Model evaluation and decision

- `crates/analyzer/examples/evaluate.rs <library folder>` samples up to N tracks (default 300). From each it derives variants: section, re-encode, remaster, and ±4% pitch.
- For each backend it measures the similarity distribution of same-recording pairs against different tracks from the same artist or label and against random pairs. It reports ROC AUC, separation (d′), the true-positive rate at a 1% false-positive rate, section consistency, pitch robustness, time and peak memory.
- Where the library contains different mixes with the same normalised artist and title, it measures version separation (the "different mixes" case).
- Output is aggregate JSON and Markdown only; no paths or names.
- The result goes in `docs/decisions/0001-analysis-model.md`, which records the chosen model, weights URL, SHA-256, licence, preprocessing (sample rate, window, hop, mel settings), segment policy, runtime and measured costs. That pinned version becomes `CURRENT_ANALYSIS_VERSION`. `docs/decisions/0002-dependencies.md` records the licences of the new crates.

## Steps (one commit each)

1. **Identity core (#8):** normalisation, mix classification, migration 0005, the policy function and unit tests for every rule.
2. **Fingerprints and fixtures (#8, #9):** the synthetic song generator, `rusty-chromaprint` integration with offset and speed-compensated matching, committed codec fixtures, `cases.json`, and the calibration test.
3. **Analysis worker (#9):** the `cd-analyzer` binary and protocol, process limits, `cd-dsp-v1`, tempo, key, loudness, segments and quality. Tests cover a crash (fault injected by an environment variable), a timeout, and a garbage-output decode that fails the job with a reason while a playback test keeps running.
4. **Feature store and versions (#9):** writing `feature_record`, the version guard, `analysis_state`, the upgrade path with needs_audio, and a test that deleting temporary audio keeps features. `AnalyseHandler` now calls the worker.
5. **Identity in the pipeline (#8):** fingerprint matching during import and acquisition, auto-merge with evidence, conflict creation, candidate-against-library matching, and the Identity review and Versions UI with component tests.
6. **Pretrained backends and evaluation (#9):** tract backends, model download with checksum pinning, the evaluation tool, a run on the user's library, the decision records, and pinning the chosen model.
7. **Benchmarks and acceptance (#8, #9):** the benchmark run, a calibration report in `docs/`, and acceptance mapping in `docs/milestone-2-plan.md`.

## Verification

- `scripts/check.sh` passes. New tests cover: the policy rules, calibration on the fixture set (zero false merges), the version guard, the upgrade path with missing audio (visible, no retry loop), crash and timeout isolation, and features surviving temporary-audio deletion.
- In the app: import a folder containing a re-encoded copy of an owned track and confirm it merges with evidence shown; import a remix and confirm it is linked as a version; create a mix conflict and resolve it in the Identity screen; watch the analysis jobs in Activity while playback continues.
- Run the evaluation on the user's library (the user supplies the folder path) and commit only the aggregate report.
- Maps to the acceptance criteria in #8 and #9. The "model decision record published" criterion depends on step 6's evaluation run.

## Known limits

- Real "different mixes" coverage in the fixtures is synthetic; the user's library provides the real check in the evaluation.
- The Windows memory limit uses a Job Object, which CI runs but I cannot test by hand.
