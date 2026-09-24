import { expect, it } from 'vitest'
import type { MapPoint } from './api'
import { colouring, DIMMED, OVERLAP, UNKNOWN } from './mapColour'

const pt = (id: string, extra: Partial<MapPoint> = {}): MapPoint => ({
  track_id: id, x: 0, y: 0, cluster: null, artist: null, title: null, mix: null,
  year: null, genre: null, label: null, release_country: null, playlists: [], ...extra,
})

it('shows noise and unknown values as their own legend entries', () => {
  const points = [pt('a', { cluster: 0 }), pt('b', { cluster: 1 }), pt('c')]
  const c = colouring(points, 'cluster', new Set())
  expect(c.legend.map((l) => [l.label, l.count])).toEqual([['Cluster 1', 1], ['Cluster 2', 1], ['No cluster (noise)', 1]])
  expect(c.colour(points[2])).toBe(UNKNOWN)
  const g = colouring([pt('a', { genre: 'House; Deep House' }), pt('b')], 'genre', new Set())
  expect(g.legend.map((l) => l.label)).toEqual(['House', 'Genre unknown'])
})

it('highlighting dims other points without changing anything else', () => {
  const points = [pt('a', { label: 'Strictly' }), pt('b', { label: 'Nervous' })]
  const before = JSON.stringify(points)
  const c = colouring(points, 'label', new Set(['Strictly']))
  expect(c.colour(points[1])).toBe(DIMMED)
  expect(c.colour(points[0])).not.toBe(DIMMED)
  expect(JSON.stringify(points)).toBe(before)
})

it('marks tracks in more than one chosen playlist', () => {
  const points = [pt('a', { playlists: ['p1', 'p2'] }), pt('b', { playlists: ['p1'] }), pt('c')]
  const names = new Map([['p1', 'Warm-up'], ['p2', 'Peak']])
  const c = colouring(points, 'playlist', new Set(['p1', 'p2']), names)
  expect(c.colour(points[0])).toBe(OVERLAP)
  expect(c.colour(points[1])).not.toBe(OVERLAP)
  expect(c.colour(points[2])).toBe(DIMMED)
  expect(c.legend.find((l) => l.key === '__overlap')?.count).toBe(1)
  expect(c.legend.map((l) => l.label)).toContain('In no playlist')
})

it('uses a year scale and greys unknown years', () => {
  const c = colouring([pt('a', { year: 1990 }), pt('b', { year: 2020 }), pt('c')], 'year', new Set())
  expect(c.years).toEqual([1990, 2020])
  expect(c.legend).toEqual([{ key: '', label: 'Release year unknown', colour: UNKNOWN, count: 1 }])
})
