# Identity calibration

How the identity policy (`crates/core/src/identity/policy.rs`) was checked against labelled cases, and the thresholds it uses. The cases live in `crates/core/tests/identity_cases.json` and run in `crates/core/tests/identity_calibration.rs` on every CI build.

## Fixture set

All audio is synthetic, from the deterministic song generator in `crates/audio/src/synth.rs`: drums, bass, chords and a melody from a seed. Each case derives the variant it needs:

- Re-encodes of one 30-second song: MP3 at 128 kbps, MP3 at 64 kbps mono, and AAC at 96 kbps. These are committed, made by `scripts/gen-identity-fixtures.sh`.
- Remasters: a tilted EQ, lower level and saturation.
- Pitched copies at +2%, -4% and +8%, changing speed and pitch together as a turntable does.
- Excerpts and an edit with a section removed.
- Remixes: same key, chords and melody, with new drums, bassline and tempo.
- Unrelated songs, including mislabelled files whose tags claim another song.

## Thresholds

| Setting | Value | Where |
| --- | --- | --- |
| Minimum alignment score for a fingerprint match | 0.6 | `MIN_ALIGNMENT_SCORE`, `Thresholds::match_score` |
| Minimum coverage for any fingerprint evidence | 0.2 | `MIN_ALIGNMENT_COVERAGE` |
| Coverage of the longer recording for "whole recording" | 0.85 | `Thresholds::full_coverage` |
| Length tolerance for a pitched copy | 1.5% | `Thresholds::pitch_tolerance` |
| Speeds searched | implied by lengths, ±0.3% | `fingerprint::compare_with_speed` |

The score is 1 minus the mean bit error of aligned Chromaprint segments divided by 16; random audio scores near 0. Coverage is the share of the longer recording that aligns, so excerpts and edits show as partial.

## Results

| Case | Expected | Result | Score | Coverage | Speed |
| --- | --- | --- | --- | --- | --- |
| re-encode MP3 128 | same_recording | same_recording | 0.98 | 1.00 | 1.000 |
| re-encode AAC 96 | same_recording | same_recording | 0.94 | 1.00 | 1.000 |
| re-encode MP3 64 mono | same_recording | same_recording | 0.97 | 1.00 | 1.000 |
| re-encode, one side untagged mix | same_recording | same_recording | 0.94 | 1.00 | 1.000 |
| remaster | same_recording | same_recording | 0.88 | 1.00 | 1.000 |
| remaster of another song | same_recording | same_recording | 0.82 | 1.00 | 1.000 |
| pitched +2% | pitched_copy | pitched_copy | 1.00 | 1.00 | 1.020 |
| pitched -4% | pitched_copy | pitched_copy | 1.00 | 1.00 | 0.960 |
| pitched +8% | pitched_copy | pitched_copy | 1.00 | 1.00 | 1.080 |
| excerpt as radio edit | different_version | different_version | 0.89 | 0.34 | 1.000 |
| excerpt with same tags | different_version | different_version | 0.93 | 0.34 | 1.000 |
| edit with a section removed | different_version | different_version | 0.98 | 0.39 | 1.000 |
| remix | different_version | different_version | 0.50 | 0.52 | 1.000 |
| remix, other song | different_version | different_version | 0.00 | 0.00 | 1.000 |
| unrelated | unrelated | unrelated | 0.00 | 0.00 | 1.000 |
| unrelated, same artist | unrelated | unrelated | 0.00 | 0.00 | 1.000 |
| mislabelled: unrelated audio, same tags | needs_review | needs_review | 0.00 | 0.00 | 1.000 |
| same audio, tags name different mixes | needs_review | needs_review | 0.98 | 1.00 | 1.000 |
| same audio, different title | needs_review | needs_review | 0.94 | 1.00 | 1.000 |
| remix labelled as the original | needs_review | needs_review | 0.00 | 0.00 | 1.000 |
| names only, no audio | unknown | unknown | - | - | - |
| names only, different mixes | different_version | different_version | - | - | - |
| names only, different songs | unrelated | unrelated | - | - | - |

All 23 cases give the expected verdict, with no false "same recording". The same-recording cases score between 0.82 and 0.98 with full coverage; unrelated songs and remixes score 0 to 0.5 or cover about half at most. Pitched copies align only after speed compensation, and then score 1.00.

## Limits

- Synthetic songs are cleaner than real recordings. The real check is the evaluation on a DJ library in milestone 2 step 6. The thresholds should be revisited with those results before unattended acquisition is enabled (#12).
- Pitch detection relies on the length ratio to estimate speed. A pitched copy that is also trimmed will go to review rather than being recognised.
