# Library map

Issue #25. The Map view places every analysed library track by how it sounds. It has two modes: a point cloud and the same cloud with nearest-neighbour links drawn in. The map is for browsing and for picking tracks for playlists or discovery seeds. It never changes playlists, ratings or metadata by itself.

## What is computed

All of it runs locally on the whole-track embeddings of the current analysis model (`crates/core/src/similarity`).

1. **Inputs.** Library tracks (a file imported or archived here) with an embedding of the current model version. Each embedding is scaled to unit length. Embeddings of other versions are never loaded, so two models are never compared. Tracks without one are listed with the reason, never given invented positions.
2. **Nearest neighbours.** Exact: every pair of tracks is compared by cosine distance on several threads, with one core left free for playback. Ties are broken by track order. No dense distance matrix is kept; each track keeps only its nearest `k` (default 10).
3. **Clusters.** DBSCAN on the same cosine distances, not on the 2D layout.
   - A track is a core point when at least `min_samples` tracks (default 5, itself included) are within `eps`.
   - Border tracks join their nearest core point, so the result does not depend on processing order.
   - Tracks that are neither are noise, shown as "No cluster (noise)" and never forced into a cluster.
   - Clusters are numbered in track id order, so the same input and settings give the same numbers.
4. **Cluster distance.** A fixed `eps` suited to every library could not be justified, so by default it is chosen from the library: the median distance from each track to its 4th nearest neighbour (`min_samples - 1`). About half the tracks are then core points. The user can set it instead. The value used and the rule that chose it are saved with the map.
5. **Layout.** UMAP's objective (fuzzy neighbour weights, min_dist 0.1, negative sampling), implemented in `compute.rs` and optimised from a PCA start with a fixed seed. Screen distance only approximates neighbourhoods. The view shows true embedding similarity for a track's neighbours, and says that positions are approximate.

The saved map records the model version, pipeline version (`map-1`), distance, preprocessing, neighbours, `min_samples`, `eps` with its rule, layout, epochs and seed. It stays until the user rebuilds it. The view says when it is stale:
- tracks were analysed or reanalysed since it was made;
- tracks left the library;
- the analysis model changed.

Rebuilding runs in the background with progress and can be cancelled; the old map is kept until the new one is saved.

## Colour, filters and selection

- **Colour by:** cluster, playlist, release year, genre, label or release country. Changing colour or highlighting never recomputes positions or clusters. The legend's highlight dims other points; it does not recluster the subset.
- **Unknown values** have their own grey legend entry. Genre colours by its first value; all values show in the inspector. More categories than the palette holds are grouped as Other; highlighting picks out any single value.
- **Playlists** are many-to-many. The legend lists every playlist. With more than one highlighted, tracks in several of them get their own overlap colour, and the inspector lists every playlist a track is in.
- **Release country** is MusicBrainz's release country (or Discogs', if MusicBrainz has none) from metadata identification. It is not the artist's nationality. It is kept as a sourced value beside the other metadata.
- **Selection:**
  - click to inspect a track, play it, and step through its nearest neighbours;
  - shift-click or shift-drag to select several;
  - add the selection to a playlist, or add the tracks as discovery seeds. Both need a click.
  - Find a track and the neighbour list give keyboard access without the canvas.

## Measurements

`similarity::tests::ten_thousand_tracks` (ignored by default) builds a map of 10,000 tracks with 1,280 dimensions, the EffNet embedding size: 40 groups of 240 tracks plus 400 scattered tracks.

| Machine | Build (neighbours, clusters, layout) | Peak memory of the test process |
| --- | --- | --- |
| Apple M4, 16 GB, release build | 5.5 s | 68 MB (51 MB of it the embeddings) |

The build runs on its own thread and database connection, so playback and the interface are not blocked. Drawing 10,000 points and panning in the app has not been measured yet; it needs a session with the app open.

## Tests

- `similarity::tests`: neighbours match a reference cosine calculation; DBSCAN finds planted groups, leaves strays as noise, is reproducible, and assigns border tracks to the nearest core track; the layout is repeatable for a seed and keeps groups together; cancelling stops a build; only the current model's embeddings are used, unplaced tracks are listed with reasons, and playlists and ratings are unchanged; provenance records the settings used; staleness is reported.
- `mapColour.test.ts`: noise and unknown categories, highlighting without changes, playlist overlap, year scale.
- `Map.test.ts`: coverage and staleness, inspecting by search, colour changes without rebuilding, seeds only on request, rebuild settings, the unplaced list.
