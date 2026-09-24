# Playlist workspaces

Issue #6, with the steering on #5, #13, #14 and #20. Playlists are where discovery starts. Each playlist is a workspace with its own brief, seeds, suggestions and feedback, next to the accepted, manually ordered track list that gets exported.

## Model

Migration `0017_playlist_workspace.sql`:

- **`playlist.brief`:** the user's description of the playlist, e.g. "Warm-up set: dubby, spacious, restrained vocals".
- **`playlist.discovery`:** whether the app looks for tracks for it in the background.
- **`playlist_seed`:** artists, labels, DJs and tracks for this playlist only.
- **`candidate_context`:** which playlists each candidate was suggested for. A track is one candidate however many playlists want it, so it is downloaded and analysed once.
- **`playlist_feedback`:**
  - "fits" or "not for this playlist", per playlist and track;
  - append-only, with undo;
  - never touches the personal rating.
- **`source_run.playlist_id`:** the searches made for each playlist, shown in its workspace.

## Behaviour

- **Discovery for a playlist** uses the playlist's seeds, then the artists and labels of its accepted tracks. Global seeds and ratings are not used. The brief is passed to the model, marked as untrusted text, when a model is set up. Results are recorded as suggestions for that playlist; tracks already known just gain the context.
- **The playlist's queue** holds suggestions with playable audio that have no decision for this playlist yet. It keeps tracks rated elsewhere, because a globally liked track still needs a decision per playlist. It leaves out tracks the user gave thumbs down to, since explicit dislikes stay global. The order is:
  - similarity to the playlist's accepted tracks (the mean of the three closest), then
  - the personal ranking score, then
  - a bonus for a playlist seed match, then
  - a penalty when the track sounds closer to one the user said does not fit than to anything accepted.
- **Adding** appends the track to the end of the playlist. Nothing else changes membership or order: refreshes, reranking and brief changes leave the accepted list alone. Undo removes only the entry the add created, even after the user has moved it.
- **Background refresh:** a playlist with discovery on asks for more when fewer than 10 suggestions are waiting, at most once an hour. Downloads, analysis and daily limits are shared across the whole app, not multiplied by the number of playlists.
- **Export** uses only the accepted list in the user's order. Suggestions are never exported.

## Interface

- **Navigation:** Playlists, Discovery, Library, Map, Activity, Identity, Settings. Playlists opens first.
- **Playlist workspace:** edit the brief and seeds, turn background discovery on or off, find tracks now, see recent searches, and open the suggestions (with a count of how many are waiting).
- **Discovery:** shows everything, or one playlist's suggestions. Each card shows the playlists it was suggested for, and the chips switch between them.
- **In a playlist's queue:**
  - A adds the track and N says it does not fit;
  - ratings stay personal and do not decide anything for the playlist;
  - Z undoes whichever came last, a rating or a playlist decision.

## Tests

`workspace::tests`:
- each playlist discovers from its own brief and seeds;
- feedback is per playlist and leaves ratings alone;
- a global thumbs down removes a track from every queue;
- adding appends, and undo removes only that entry;
- order and context survive a restart and a refresh;
- tracks that sound like the playlist rank first;
- only playlists with discovery on and a short queue refresh.

UI: `PlaylistWorkspace.test.ts` and the playlist cases in `Review.test.ts`.
