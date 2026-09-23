/** Format milliseconds as m:ss, or h:mm:ss past an hour. */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return '-:--'
  const total = Math.floor(ms / 1000)
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = String(total % 60).padStart(2, '0')
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`
}

/** Human-readable byte size. */
export function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`
  return `${bytes} B`
}

/** Short relative time such as "5 s ago" or "in 2 min". */
export function formatRelative(ms: number, now = Date.now()): string {
  const diff = Math.round((ms - now) / 1000)
  const abs = Math.abs(diff)
  const text =
    abs < 60 ? `${abs} s` : abs < 3600 ? `${Math.round(abs / 60)} min` : abs < 86400 ? `${Math.round(abs / 3600)} h` : `${Math.round(abs / 86400)} d`
  if (abs < 2) return 'now'
  return diff < 0 ? `${text} ago` : `in ${text}`
}

const RATING_LABELS: Record<string, string> = {
  thumbs_down: 'Thumbs down',
  star1: '★',
  star2: '★★',
  star3: '★★★',
}

export function formatRating(kind: string | null | undefined): string {
  return kind ? (RATING_LABELS[kind] ?? kind) : ''
}

/** "Artist - Title (Mix)" with graceful fallbacks. */
export function trackLabel(t: { artist: string | null; title: string | null; mix: string | null }): string {
  const title = t.title ?? 'Untitled'
  const withMix = t.mix ? `${title} (${t.mix})` : title
  return t.artist ? `${t.artist} - ${withMix}` : withMix
}

const CODECS: Record<string, string> = {
  pcm_s16le: 'WAV',
  pcm_s24le: 'WAV 24-bit',
  pcm_s16be: 'AIFF',
  pcm_s24be: 'AIFF 24-bit',
  flac: 'FLAC',
  mp3: 'MP3',
  aac: 'AAC',
  alac: 'ALAC',
  vorbis: 'Ogg Vorbis',
}

/** Readable name for a decoder codec identifier. */
export function formatCodec(codec: string | null | undefined): string {
  if (!codec) return 'Unknown format'
  return CODECS[codec] ?? codec.toUpperCase()
}

/** "pitched:+4.0" as "Pitched +4.0%". */
export function formatVariant(variant: string | null | undefined): string {
  if (!variant) return ''
  const [kind, value] = variant.split(':')
  if (kind === 'pitched') return `Pitched ${value}%`
  return kind
}
