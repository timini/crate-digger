# Ranking calibration

How the Tier 2 ranking (`crates/core/src/ranking/mod.rs`) works and how its weights were chosen. The calibration runs in `crates/core/tests/ranking_calibration.rs` on every CI build; the build fails if `ranking::DEFAULT` differs from the weights it selects, if personal ranking stops beating cultural evidence alone, or if the smaller of two liked styles drops out of the top 20.

**These numbers come from synthetic listeners, not real ratings.** No ratings exist yet. The weights and thresholds suit the synthetic embedding space and must be checked again once real ratings have been collected (see "Next calibration").

## How a candidate is scored

- **Taste.** Among the candidate's five nearest rated tracks (by embedding, current analysis version only), the share the listener liked, weighted by similarity and star rating, times the mean similarity to its three nearest likes. A style the listener mostly likes scores high even if a few of its tracks were rated down.
- **Dislikes.** A disliked track lowers a candidate only when it is closer than every liked track, so copies and near copies of a disliked version are suppressed while artists, labels and whole styles are not.
- **Evidence.** The strongest source confidence for the candidate (Discogs, a page, pasted text).
- **Cold start.** Until five positive ratings exist, the score blends towards 0.7 × evidence + 0.3 × seed match (artist or label is a seed), in proportion to the number of positives.
- **Taste clusters.** Liked tracks are grouped by embedding (a track joins the cluster whose centre is at least 0.3 similar). Each candidate belongs to the cluster of its nearest like.
- **Queue order.** Every fifth review position goes to an exploration pick: a candidate with source evidence of at least 0.5 whose taste is below 0.5. The other positions go by score, with 0.15 taken off for each of the last four picks from the same cluster and 0.15 for an artist heard in the last three. No exploration picks are made during cold start.
- **When it runs.** After every rating, skip or undo (the rating is saved first; reranking runs in the background and never touches the player), and within a minute of new tracks becoming ready.

## Synthetic listeners

Each world has six styles with random centres in 64 dimensions; each track is its style's centre plus noise, giving a similarity of about 0.45 within a style and about 0 between styles. The listener likes 85% of styles 0 and 1, 30% of style 2, 5% of style 3, 40% of style 4 and 20% of style 5. Source evidence favours styles 0, 1 and 4. The listener has rated 60 tracks; the remaining 740 are candidates.

Weights were chosen by grid search on five training worlds (seeds 1 to 5), maximising precision at 20 plus precision at 50 plus the share of the smaller liked style in the top 20, then reported on five independent held-out worlds (seeds 11 to 15).

## Results

Chosen on five training worlds: w_taste 0.6, w_evidence 0.5, cluster_threshold 0.3, cluster_repeat_cost 0.15, neighbours 5.

| Five held-out worlds (mean) | Precision at 20 | Precision at 50 | Smaller liked style in top 20 | Disliked style in top 20 |
| --- | --- | --- | --- | --- |
| Chosen weights, taste clusters | 0.85 | 0.83 | 0.48 | 0 |
| One average vector, same weights | 0.81 | 0.78 | 0.08 | 0 |
| Cultural evidence only | 0.82 | 0.81 | 0.48 | 0 |

Scoring and ordering 1,000 candidates against 300 positives at 1,280 dimensions took 0.17 s (release build).

A full database rerank (loading embeddings and ratings, scoring, ordering and writing the queue) of 1,000 ready candidates against 300 rated tracks at 1,280 dimensions took 0.19 s in a release build on an Apple M4 (`ranking::tests::rerank_timing_for_a_thousand_candidates`, ignored in CI).

## Reading the results

- Taste clusters keep both liked styles in the top 20 (about half each); one average vector of all likes gives the smaller style almost no places.
- Personal ranking beats cultural evidence alone, but only slightly here, because the synthetic sources already favour the liked styles. Real sources will be noisier.
- Earlier formulations failed this calibration and were changed: nearest-single-like taste let one stray like promote a whole style; the maximum similarity to any dislike penalised styles the listener mostly likes; single-linkage clustering chained unrelated styles together. They are recorded here so they are not tried again without new evidence.

## Next calibration

Once a few hundred real ratings exist, repeat the selection on real data: hold out the most recent 20% of ratings, rank the held-out tracks from the rest, and compare precision at 20 and cluster coverage with the synthetic figures above. The cluster threshold in particular depends on how the chosen model's embeddings spread.
