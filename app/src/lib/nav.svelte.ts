// Which view is open, and which playlist's suggestions Discovery shows.
export type View = 'playlists' | 'review' | 'library' | 'map' | 'identity' | 'activity' | 'settings'

export const nav = $state({
  view: 'playlists' as View,
  /** Discovery shows this playlist's suggestions; null shows everything. */
  reviewPlaylist: null as string | null,
})

/** Open Discovery on one playlist's suggestions, or on everything. */
export function reviewFor(playlist: string | null) {
  nav.reviewPlaylist = playlist
  nav.view = 'review'
}
