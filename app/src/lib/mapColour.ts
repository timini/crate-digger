// Colouring for the library map. Colour never changes positions or clusters.
import type { MapPoint } from './api'

export type ColourMode = 'cluster' | 'playlist' | 'year' | 'genre' | 'label' | 'country'

export const UNKNOWN = '#8a8f98'
export const DIMMED = 'rgba(138, 143, 152, 0.18)'
export const OVERLAP = '#f5f5f5'
// Distinguishable categorical colours; beyond these, values are grouped as Other.
export const PALETTE = [
  '#4e79a7', '#f28e2b', '#e15759', '#76b7b2', '#59a14f', '#edc948',
  '#b07aa1', '#ff9da7', '#9c755f', '#17becf', '#bcbd22', '#aec7e8',
]
const OTHER = '#5b5f66'

export interface LegendEntry { key: string; label: string; colour: string; count: number }

/** The first of several values, as used for colour; all values stay visible in the inspector. */
export function firstValue(v: string | null): string | null {
  const first = v?.split(/[;,/]/)[0]?.trim()
  return first ? first : null
}

function categoryOf(p: MapPoint, mode: ColourMode): string | null {
  switch (mode) {
    case 'cluster': return p.cluster === null ? null : String(p.cluster)
    case 'genre': return firstValue(p.genre)
    case 'label': return p.label?.trim() || null
    case 'country': return p.release_country?.trim() || null
    default: return null
  }
}

/** Year as a colour from a fixed scale; unknown years are grey. */
export function yearColour(year: number | null, min: number, max: number): string {
  if (year === null) return UNKNOWN
  const t = max > min ? (year - min) / (max - min) : 0.5
  const hue = 260 - 220 * t // blue for older, orange for newer
  return `hsl(${hue.toFixed(0)}, 70%, 55%)`
}

export interface Colouring {
  colour: (p: MapPoint) => string
  legend: LegendEntry[]
  /** For year mode, the range shown on the scale. */
  years?: [number, number]
}

/**
 * Colours and legend for a mode. `highlight` limits colour to the chosen
 * legend keys (other points are dimmed); for playlists it is the chosen
 * playlists, and tracks in more than one of them get the overlap colour.
 */
export function colouring(
  points: MapPoint[],
  mode: ColourMode,
  highlight: Set<string>,
  playlistNames: Map<string, string> = new Map(),
): Colouring {
  if (mode === 'year') {
    const years = points.map((p) => p.year).filter((y): y is number => y !== null)
    const [min, max] = years.length ? [Math.min(...years), Math.max(...years)] : [0, 0]
    const unknown = points.length - years.length
    return {
      colour: (p) => yearColour(p.year, min, max),
      legend: [{ key: '', label: 'Release year unknown', colour: UNKNOWN, count: unknown }],
      years: years.length ? [min, max] : undefined,
    }
  }
  if (mode === 'playlist') {
    const counts = new Map<string, number>()
    let none = 0
    for (const p of points) {
      if (!p.playlists.length) none++
      for (const id of p.playlists) counts.set(id, (counts.get(id) ?? 0) + 1)
    }
    const ids = [...counts.keys()].sort((a, b) => (counts.get(b)! - counts.get(a)!) || a.localeCompare(b))
    const colourOf = new Map(ids.map((id, i) => [id, PALETTE[i % PALETTE.length]]))
    const legend: LegendEntry[] = ids.map((id) => ({
      key: id, label: playlistNames.get(id) ?? 'Playlist', colour: colourOf.get(id)!, count: counts.get(id)!,
    }))
    legend.push({ key: '', label: 'In no playlist', colour: UNKNOWN, count: none })
    const overlap = points.filter((p) => p.playlists.filter((id) => highlight.has(id)).length > 1).length
    if (highlight.size > 1) legend.push({ key: '__overlap', label: 'In more than one chosen playlist', colour: OVERLAP, count: overlap })
    return {
      legend,
      colour: (p) => {
        const chosen = p.playlists.filter((id) => highlight.size === 0 || highlight.has(id))
        if (chosen.length > 1 && highlight.size > 1) return OVERLAP
        if (chosen.length >= 1) return colourOf.get(chosen[0])!
        return highlight.size ? DIMMED : UNKNOWN
      },
    }
  }
  const counts = new Map<string, number>()
  let unknown = 0
  for (const p of points) {
    const c = categoryOf(p, mode)
    if (c === null) unknown++
    else counts.set(c, (counts.get(c) ?? 0) + 1)
  }
  const keys = [...counts.keys()].sort((a, b) =>
    mode === 'cluster' ? Number(a) - Number(b) : (counts.get(b)! - counts.get(a)!) || a.localeCompare(b))
  const shown = new Set(keys.slice(0, PALETTE.length))
  const colourOf = new Map(keys.map((k, i) => [k, shown.has(k) ? PALETTE[i % PALETTE.length] : OTHER]))
  const legend: LegendEntry[] = keys.map((k) => ({
    key: k,
    label: mode === 'cluster' ? `Cluster ${Number(k) + 1}` : k,
    colour: colourOf.get(k)!,
    count: counts.get(k)!,
  }))
  const unknownLabel = {
    cluster: 'No cluster (noise)', genre: 'Genre unknown', label: 'Label unknown', country: 'Release country unknown',
  }[mode as 'cluster' | 'genre' | 'label' | 'country']
  legend.push({ key: '', label: unknownLabel, colour: UNKNOWN, count: unknown })
  return {
    legend,
    colour: (p) => {
      const c = categoryOf(p, mode) ?? ''
      if (highlight.size && !highlight.has(c)) return DIMMED
      return c === '' ? UNKNOWN : colourOf.get(c)!
    },
  }
}
