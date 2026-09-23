# Download matching calibration

How the rule that chooses a Soulseek download (`crates/core/src/acquisition/matching.rs`) was checked, and what it does. The labelled cases are in `crates/core/tests/acquisition_cases.json` and run in `crates/core/tests/acquisition_calibration.rs` on every CI build. The build fails if the rule would ever start a download of a file that is not the wanted recording.

## The rule

A search result is acceptable when:

- the title part of its file name is the wanted title (a longer title that contains it does not count);
- a main artist is named somewhere in its path;
- its mix fits: the same mix, or both the original (an unnamed file counts as the original when the original is wanted). A file that does not name the wanted mix, or a longer or shorter cut of the original, is "uncertain" and goes to the user;
- it is not marked as a preview, snippet, sample, pitched, sped up, slowed, live, karaoke or a cut from a DJ mix, and is at least 60 seconds long;
- it is lossless (FLAC, WAV, AIFF, ALAC, APE, WavPack) or an MP3 of at least 320 kbps.

A download starts without asking only when unattended downloads are on, at least one result is acceptable, and every acceptable result lists its length and the lengths agree within 10 seconds. Otherwise the results go to the "choose a download" list, ranked: acceptable first, then by mix fit, quality, free upload slot, queue length and upload speed. Unattended downloads are off by default; with them off, a clear match is shown as the recommended choice.

After download, validation decodes the file and compares its length with the listed length (5 seconds tolerance) before it can reach the review queue.

## Cases

The cases follow the categories of the identity fixture set (`docs/identity-calibration.md`): re-encodes, remasters, pitched copies, excerpts, edits, remixes, unrelated songs and mislabelled files, written as the result lists Soulseek returns.

| Case | Expected | Result | Detail |
| --- | --- | --- | --- |
| re-encodes: FLAC and MP3 320 | auto | auto | downloads 01 - Alpha Unit - First Light.flac |
| re-encodes below 320 only | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| remaster of the original | auto | auto | downloads Alpha Unit - First Light (2021 Remaster).flac |
| pitched copy only | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| pitched copy beside the original | auto | auto | downloads Alpha Unit - First Light.flac |
| excerpt beside the original | auto | auto | downloads Alpha Unit - First Light.mp3 |
| excerpt with no marker | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| radio edit when the original is wanted | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| someone's edit when the original is wanted | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| remix wanted, original also shared | auto | auto | downloads Alpha Unit - First Light (Beta Remix).mp3 |
| original wanted, only remixes shared | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| another remixer's version | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| same title by another artist beside the right one | auto | auto | downloads Alpha Unit - First Light.flac |
| same title by another artist only | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| longer title containing the wanted one | nothing | nothing | No result names this track. |
| mislabelled: same name, different lengths | choose | choose | Matching copies differ in length by 120 s, so they may be different versions. |
| cut from a DJ mix beside the original | auto | auto | downloads Alpha Unit - First Light.flac |
| live recording only | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| extended mix wanted, file does not say | choose | choose | No copy meets the automatic rule (right mix, lossless or 320 kbps). |
| extended mix wanted and named | auto | auto | downloads Alpha Unit - First Light (Extended Mix) [FIX001].flac |
| featured artist written differently | auto | auto | downloads Alpha Unit ft. Singer - First Light.flac |
| artist only in the folder | auto | auto | downloads A1 First Light.flac |
| no length listed | choose | choose | No copy lists its length, so the version cannot be checked. |
| title in a folder only | nothing | nothing | No result names this track. |
| no results | nothing | nothing | Soulseek returned no results. |

25 cases: 10 automatic, 12 sent to the user, 3 with nothing usable. Wrong automatic downloads: 0. Recommendations that were the right file: 2 of 2.

## Limits

These cases are constructed, not sampled from real Soulseek searches, so they show that the rule behaves as designed on each category rather than how often real searches reach each outcome. File names on Soulseek are written by the people sharing them; a correctly named file can still hold the wrong audio. Validation catches wrong lengths, and fingerprint matching after analysis links a download to any copy already in the library, but a new track has nothing to compare against. That is why unattended downloads stay off until the user turns them on.
