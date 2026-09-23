// Typed wrappers around Tauri commands. Rust command names are snake_case;
// Tauri converts camelCase argument names to snake_case automatically.
import { invoke } from '@tauri-apps/api/core'

export interface AppInfo {
  version: string
  schema_version: number
  data_dir: string
  db_path: string
}

export type JobState = 'queued' | 'running' | 'paused' | 'blocked' | 'failed' | 'cancelled' | 'done'
export type HoldCode = 'user' | 'quit' | 'connector_auth' | 'daily_limit' | 'storage_limit'

export interface Job {
  id: string
  kind: string
  connector: string | null
  state: JobState
  hold_code: HoldCode | null
  reason: string | null
  payload: unknown
  attempts: number
  max_attempts: number
  next_run_at: number
  created_at: number
  updated_at: number
}

export interface JobCount {
  kind: string
  state: JobState
  count: number
}

export interface SchedulerStatus {
  accepting: boolean
  playback_active: boolean
  staging_used_bytes: number
  staging_budget_bytes: number
  daily: { kind: string; used: number; limit: number }[]
}

export interface ConnectorHealth {
  connector: string
  status: 'ok' | 'auth_failed' | 'unavailable'
  reason: string | null
  updated_at: number
}

export interface Activity {
  jobs: Job[]
  counts: JobCount[]
  scheduler: SchedulerStatus
  connectors: ConnectorHealth[]
}

export type Availability = 'available' | 'missing' | 'corrupt'
export type Field =
  | 'artist'
  | 'title'
  | 'mix'
  | 'label'
  | 'release'
  | 'track_number'
  | 'year'
  | 'genre'
  | 'tempo'
  | 'musical_key'
export type RatingKind = 'thumbs_down' | 'star1' | 'star2' | 'star3'
export type RatingFilter = 'unrated' | 'thumbs_down' | { min_stars: number }
export type SortBy = 'artist' | 'title' | 'tempo' | 'key' | 'added' | 'rating'

export interface LibraryQuery {
  text?: string | null
  artist?: string | null
  title?: string | null
  mix?: string | null
  label?: string | null
  rating?: RatingFilter | null
  tempo_min?: number | null
  tempo_max?: number | null
  key?: string | null
  availability?: Availability | null
  playlist_id?: string | null
  sort?: SortBy
  descending?: boolean
  limit?: number
  offset?: number
}

export interface LibraryRow {
  track_id: string
  artist: string | null
  title: string | null
  mix: string | null
  label: string | null
  release: string | null
  year: number | null
  genre: string | null
  tempo: number | null
  musical_key: string | null
  file_id: string | null
  path: string | null
  availability: Availability | null
  availability_reason: string | null
  duration_ms: number | null
  format: string | null
  file_count: number
  rating: RatingKind | null
  playlist_count: number
  analysed: boolean
  kept: boolean
  added_at: number
}

export interface LibraryPage {
  rows: LibraryRow[]
  total: number
}

export interface LibraryRoot {
  id: string
  path: string
  created_at: number
  file_count: number
}

export interface TrackMeta {
  artist: string | null
  title: string | null
  mix: string | null
  label: string | null
  release: string | null
  track_number: string | null
  year: number | null
  genre: string | null
  tempo: number | null
  musical_key: string | null
}

export interface FieldProvenance {
  field: Field
  value: string | null
  source: string
  corrected: boolean
}

export interface FileRecord {
  id: string
  track_id: string
  path: string
  origin: 'imported' | 'staged' | 'archived'
  size_bytes: number
  duration_ms: number | null
  format: string | null
  sample_rate: number | null
  channels: number | null
  bitrate_kbps: number | null
  availability: Availability
  availability_reason: string | null
  is_primary: boolean
  variant: string | null
}

export interface Version {
  track_id: string
  meta: TrackMeta
  has_audio: boolean
}

export interface KeyEstimate {
  name: string
  camelot: string
  confidence: number
}

export interface AnalysisSummary {
  model: string
  state: { state: 'queued' | 'done' | 'needs_audio' | 'failed'; reason: string | null } | null
  tempo: number | null
  tempo_confidence: number | null
  key: KeyEstimate | null
  loudness_lufs: number | null
  quality: { decoded_fraction: number | null; clipping_ratio: number; silence_ratio: number; decode_errors: number } | null
}

export interface TrackDetail {
  track_id: string
  meta: TrackMeta
  provenance: FieldProvenance[]
  files: FileRecord[]
  rating: RatingKind | null
  kept: boolean
  playlists: [string, string][]
  versions: Version[]
  analysis: AnalysisSummary
}

export type Relation = 'same_recording' | 'different_version' | 'unrelated'

export interface ConflictSide {
  track_id: string
  meta: TrackMeta
  path: string | null
  duration_ms: number | null
}

export type IdentityEvidence =
  | { kind: 'fingerprint_match'; score: number; coverage: number; speed: number }
  | { kind: 'fingerprint_mismatch'; score: number }
  | { kind: 'identical_bytes' }
  | { kind: 'embedding_similarity'; similarity: number; model: string }
  | { kind: 'llm_assertion'; claim: string }
  | { kind: 'user_decision'; relation: Relation }

export interface Conflict {
  id: string
  a: ConflictSide
  b: ConflictSide
  reason: string
  evidence: IdentityEvidence[]
  created_at: number
}

export interface ImportSummary {
  total: number
  processed: number
  added: number
  updated: number
  unchanged: number
  duplicate_copies: number
  relinked: number
  corrupt: number
  marked_missing: number
  errors: string[]
}

export interface RelinkProposal {
  file_id: string
  track_id: string
  old_path: string
  new_path: string
  method: 'content' | 'name_and_duration'
}

export interface DuplicatePair {
  track_a: string
  track_b: string
  artist: string | null
  title: string | null
  mix: string | null
  duration_a_ms: number | null
  duration_b_ms: number | null
  path_a: string | null
  path_b: string | null
}

export type PlayState = 'idle' | 'playing' | 'paused' | 'ended' | 'error'

export interface NowPlaying {
  track_id: string | null
  state?: PlayState
  path?: string | null
  position_ms?: number
  duration_ms?: number | null
  volume?: number
  error?: string | null
  underruns?: number
  output?: string
}

export interface Playlist {
  id: string
  name: string
  track_count: number
  duration_ms: number
  created_at: number
  updated_at: number
}

export interface PlaylistEntry {
  position: number
  track_id: string
  artist: string | null
  title: string | null
  mix: string | null
  tempo: number | null
  musical_key: string | null
  duration_ms: number | null
  path: string | null
  availability: Availability | null
  rating: RatingKind | null
}

export interface Evidence {
  source_kind: string
  source_url: string | null
  supplied_text_id: string | null
  retrieved_at: number
  excerpt: string
  confidence: number
}

export interface YoutubeLink {
  video_id: string
  url: string
  title: string | null
  channel: string | null
  duration_ms: number | null
  confidence: number
  preferred: boolean
  user_corrected: boolean
}

export interface ReviewCard {
  candidate_id: string
  track_id: string
  meta: TrackMeta
  file: FileRecord
  reasons: string[]
  evidence: Evidence[]
  youtube: YoutubeLink[]
  confidence: number | null
  verified: boolean
  kept: boolean
  playlists: [string, string][]
}

export interface QueueStats {
  ready: number
  in_progress: number
  skipped: number
  needs_review: number
  failed: number
  reviewed: number
}

export interface Undone {
  track_id: string
  kind: RatingKind | 'skip'
  effective: RatingKind | null
}

export interface StorageStatus {
  staging_dir: string
  archive_dir: string
  staging_used_bytes: number
  staging_budget_bytes: number
}

export interface ClearSummary {
  removed_files: number
  freed_bytes: number
  retained: number
  unreviewed_skipped: number
}

export interface UserLimits {
  ready_target: number
  replenish_below: number
  active_downloads: number
  analysis_jobs: number
  temp_budget_gb: number
  daily_acquisitions: number
  source_refresh_hours: number
}

export interface AppSettings {
  archive_dir: string
  archive_dir_is_default: boolean
  staging_dir: string
  limits: UserLimits
  demo_discovery: boolean
  close_to_tray: boolean
  onboarded: boolean
  data_dir: string
}

export interface ModelStatus {
  id: string
  licence: string
  size_bytes: number
  dims: number
  installed: boolean
  chosen: boolean
  recommended: boolean
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  activity: (states: JobState[], limit = 200) => invoke<Activity>('activity', { states, limit }),
  pauseAll: () => invoke<number>('jobs_pause_all'),
  resumeAll: () => invoke<number>('jobs_resume_all'),
  cancelJob: (id: string) => invoke<void>('job_cancel', { id }),
  retryJob: (id: string) => invoke<void>('job_retry', { id }),

  roots: () => invoke<LibraryRoot[]>('library_roots'),
  addRoot: (path: string) => invoke<string>('library_add_root', { path }),
  removeRoot: (rootId: string) => invoke<void>('library_remove_root', { rootId }),
  rescan: () => invoke<number>('library_rescan'),
  search: (query: LibraryQuery) => invoke<LibraryPage>('library_search', { query }),
  trackDetail: (trackId: string) => invoke<TrackDetail>('track_detail', { trackId }),
  setField: (trackId: string, field: Field, value: string | null) =>
    invoke<void>('track_set_field', { trackId, field, value }),
  setPrimaryFile: (fileId: string) => invoke<void>('file_set_primary', { fileId }),
  relinkFind: (folder: string) => invoke<RelinkProposal[]>('relink_find', { folder }),
  relinkApply: (fileId: string, path: string) => invoke<void>('relink_apply', { fileId, path }),
  checkFiles: () => invoke<ImportSummary>('library_check_files'),
  duplicates: () => invoke<DuplicatePair[]>('duplicates_list'),
  mergeDuplicates: (keep: string, remove: string) => invoke<void>('duplicates_merge', { keep, remove }),
  dismissDuplicates: (a: string, b: string) => invoke<void>('duplicates_dismiss', { a, b }),

  playTrack: (trackId: string, startMs?: number) => invoke<void>('player_play_track', { trackId, startMs }),
  togglePlay: () => invoke<void>('player_toggle'),
  pause: () => invoke<void>('player_pause'),
  seek: (ms: number) => invoke<void>('player_seek', { ms: Math.max(0, Math.round(ms)) }),
  setVolume: (volume: number) => invoke<void>('player_set_volume', { volume }),
  playerStatus: () => invoke<NowPlaying>('player_status'),
  waveform: (trackId: string) => invoke<number[] | null>('track_waveform', { trackId }),

  playlists: () => invoke<Playlist[]>('playlists_list'),
  createPlaylist: (name: string) => invoke<string>('playlist_create', { name }),
  renamePlaylist: (id: string, name: string) => invoke<void>('playlist_rename', { id, name }),
  deletePlaylist: (id: string) => invoke<void>('playlist_delete', { id }),
  playlistEntries: (id: string) => invoke<PlaylistEntry[]>('playlist_entries', { id }),
  addToPlaylist: (id: string, trackIds: string[], at?: number) => invoke<void>('playlist_add', { id, trackIds, at }),
  removeFromPlaylist: (id: string, position: number) => invoke<void>('playlist_remove', { id, position }),
  movePlaylistEntry: (id: string, from: number, to: number) => invoke<void>('playlist_move', { id, from, to }),

  reviewNext: (limit = 5) => invoke<ReviewCard[]>('review_next', { limit }),
  reviewStats: () => invoke<QueueStats>('review_stats'),
  rate: (trackId: string, kind: RatingKind) => invoke<string>('review_rate', { trackId, kind }),
  skip: (trackId: string) => invoke<string>('review_skip', { trackId }),
  undo: () => invoke<Undone | null>('review_undo'),
  keep: (trackId: string, keep: boolean) => invoke<void>('review_keep', { trackId, keep }),
  findMore: () => invoke<void>('review_find_more'),
  demoDiscovery: () => invoke<boolean>('demo_discovery_get'),
  setDemoDiscovery: (enabled: boolean) => invoke<void>('demo_discovery_set', { enabled }),
  storageStatus: () => invoke<StorageStatus>('storage_status'),
  clearStaging: (includeUnreviewed: boolean) => invoke<ClearSummary>('staging_clear', { includeUnreviewed }),

  settings: () => invoke<AppSettings>('settings_get'),
  setArchiveDir: (path: string | null) => invoke<void>('settings_set_archive_dir', { path }),
  setStagingDir: (path: string) => invoke<void>('settings_set_staging_dir', { path }),
  setLimits: (limits: UserLimits) => invoke<void>('settings_set_limits', { limits }),
  setCloseToTray: (enabled: boolean) => invoke<void>('settings_set_close_to_tray', { enabled }),
  completeOnboarding: () => invoke<void>('onboarding_complete'),

  conflicts: () => invoke<Conflict[]>('identity_conflicts'),
  conflictCount: () => invoke<number>('identity_conflict_count'),
  resolveConflict: (conflictId: string, relation: Relation) =>
    invoke<unknown>('identity_resolve', { conflictId, relation }),

  models: () => invoke<ModelStatus[]>('models_list'),
  downloadModel: (id: string) => invoke<void>('model_download', { id }),
  downloadProgress: () => invoke<{ id: string; bytes: number } | null>('model_download_progress'),
  chooseModel: (id: string | null) => invoke<void>('model_choose', { id }),
}
