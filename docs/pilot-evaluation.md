# Pilot evaluation protocol

Issue #22. This protocol was written, and the report that measures it was built, before any pilot data was collected. The metrics, the unit of analysis and the success criterion below are fixed. If anything changes after data arrives, the change and the reason go in the results document next to the original plan, and the planned analysis is still reported.

## Question

Does ranking discovered tracks with the DJ's own ratings (Tier 1 cultural evidence plus Tier 2 audio taste) put the tracks they like higher than cultural evidence alone (Tier 1)?

The result is published whether the combined ranking wins, ties or loses.

## Participants

- DJs who agree to take part after reading what is collected (below). No payment is needed for the result to count. Any DJ may stop at any time and ask for their report to be removed from the results.
- Aim for at least 5 DJs covering different styles. Each needs at least 200 first judgements, so that a single DJ's interval is informative.
- Each DJ uses their own computer. The report records the app and model versions; DJs are asked to add their operating system, processor and memory when they send it.

## What is collected

Only the report file that the app saves from Settings, Pilot evaluation, Save report. It contains counts and rates, with no track names, artists, file paths, seeds or ratings of individual tracks (`cd-core` `evaluation::tests::the_report_holds_no_names_or_ids`). Nothing is sent automatically. The DJ decides whether to share the file.

## Setup

- One pinned app release for the whole pilot. The analysis model is `discogs-effnet-bsdynamic-1`, the model with the best section and version separation in `docs/evaluation/embeddings-2026-09-23.md`. DJs do not change the model or the ranking settings during the pilot.
- Ranking uses `ranking::DEFAULT` (see `docs/ranking-calibration.md`). Exploration picks stay on (every fifth queue position).
- Each DJ imports their library, adds seeds (artists, labels, DJs), and connects Discogs and Soulseek as they would normally.

## Procedure

1. Use the Review queue as normal for four weeks, or until 200 or more tracks have been judged.
2. Judge each track honestly: thumbs down, one, two or three stars, or skip. Keep tracks you want in your archive.
3. Press Wrong version (W) when the audio is a different mix, edit or take from the one the sources describe. This does not change the ranking.
4. At the end, save the report and send it.

## Unit of analysis and held-out ratings

- The unit is the **first judgement** of each discovered track: its first rating or skip that was not undone. Later changes of mind are not counted.
- Every first judgement is predicted by **replay**: the app rebuilds the taste profile from the ratings that existed just before that judgement, excluding the judged track itself, and scores the track with that profile. So no judgement is ever predicted by a model that was fitted on it (`evaluation::tests::replay_uses_only_earlier_ratings_and_shows_what_taste_adds`).
- The cultural-only score is the same scoring with an empty profile: cultural evidence and seed matches.
- Different copies of one recording are merged into one track by identity matching before the pilot, so a copy cannot be judged separately from its original. Different mixes and versions are separate tracks; learning from one to predict another is what the ranking is for.

## Metrics

Primary:
- **Ranking AUC** for strong positives (two or three stars) against all other first judgements: the probability that a strong positive scores above another judged track. It is computed for cultural-only and for combined scores, together with a 95% bootstrap interval (1,000 resamples, fixed seed) of combined minus cultural.

Secondary, all in the report:
- Strong-positive rate among the top fifth of judged tracks by each score.
- Strong-positive rate, keep rate and wrong-version rate over all first judgements.
- Ready-queue availability: the share of queue samples with any ready track, and with at least the replenish threshold.
- Discovery source runs by outcome: found, empty, failed.
- Analysis outcomes with the pilot model: done, failed, waiting for audio.
- Embedding stability: cosine similarity between analyses of different copies of the same track.

Measured separately on reference hardware with `crates/analyzer/examples/evaluate.rs`, not in the report: extraction time per track and peak memory. Recovery after a crash is covered by the recovery tests.

## Analysis plan

1. Per DJ: report the two AUCs and the interval of the difference, the top-fifth rates, and the secondary metrics.
2. Pooled: the mean over DJs of the per-DJ AUC difference, with a 95% bootstrap interval that resamples DJs.
3. **Success criterion:** the lower bound of the pooled interval is above 0. Otherwise the result is "no demonstrated improvement", reported as such.
4. DJs with fewer than 200 first judgements are reported separately and left out of the pooled figure.

## Known limitations

- **Exposure bias.** DJs only judge what the queue showed them, and the queue was ordered by the combined ranking. Tracks the combined ranking placed low were shown less, which can favour it. Exploration picks (one in five) reduce this but do not remove it. A randomised comparison would need a second queue order and is outside this pilot.
- **Seeds are taken as they are at the end.** The app does not keep a history of seed changes, so seed matches in the replay use the final seeds.
- **No unpersonalised audio arm.** The comparison is cultural-only against cultural plus personal taste. Ranking by audio similarity to seeds alone is not measured.
- **Small, self-selected group.** Results describe these DJs, not DJs in general.

## Publishing

Results go in `docs/pilot-results.md`, including:
- every DJ's report, anonymised;
- the per-DJ and pooled figures;
- hardware and costs;
- the limitations above;
- any deviation from this plan.
