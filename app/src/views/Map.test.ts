import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import { afterEach, beforeAll, expect, it, vi } from 'vitest'

const calls: { cmd: string; args: unknown }[] = []
const point = (id: string, extra: object) => ({
  track_id: id, x: Number(id.slice(1)), y: 0, cluster: null, artist: 'Artist', title: `Title ${id}`, mix: null,
  year: null, genre: null, label: null, release_country: null, playlists: [], ...extra,
})
const view = {
  map: {
    id: 'm1', version: { model_id: 'effnet', weights_checksum: 'sha256:1', preprocessing_version: 'p1' },
    provenance: { pipeline_version: 'map-1', distance: 'cosine', preprocessing: 'unit-length whole-track embeddings', neighbours: 10, min_samples: 5, eps: 0.12, eps_rule: 'median distance to the 4th nearest neighbour', layout: 'UMAP objective', epochs: 200, seed: 42 },
    placed: 3, clusters: 1, noise: 1, build_ms: 1200, created_at: Date.now(),
  },
  coverage: { eligible: 4, embedded: 3, stale: 'Tracks changed since the map was made (1 newly analysed).' },
  points: [point('t1', { cluster: 0, playlists: ['p1'] }), point('t2', { cluster: 0 }), point('t3', {})],
  building: null,
  last_error: null,
}
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  if (cmd === 'map_get') return view
  if (cmd === 'playlists_list') return [{ id: 'p1', name: 'Warm-up', track_count: 1, duration_ms: 0, created_at: 0, updated_at: 0 }]
  if (cmd === 'map_neighbours') return [{ track_id: 't2', rank: 0, distance: 0.08, similarity: 0.92, artist: 'Artist', title: 'Title t2', mix: null }]
  if (cmd === 'map_unplaced') return [{ track_id: 't4', artist: 'Artist', title: 'Title t4', reason: 'Waiting for analysis.' }]
  if (cmd === 'discovery_seeds') return [{ kind: 'artist', value: 'Existing' }]
  if (cmd === 'map_edges') return []
  return null
}) }))
import LibraryMap from './Map.svelte'

beforeAll(() => {
  HTMLCanvasElement.prototype.getContext = (() => null) as never
})
afterEach(() => { cleanup(); calls.length = 0 })

it('reports coverage, staleness and noise, and inspects a track found by search', async () => {
  render(LibraryMap)
  expect(await screen.findByText(/3 of 4 library tracks/)).toBeTruthy()
  expect(screen.getByText(/1 newly analysed/)).toBeTruthy()
  expect(screen.getByText('No cluster (noise)')).toBeTruthy()
  await fireEvent.input(screen.getByLabelText('Find a track'), { target: { value: 'title t1' } })
  await fireEvent.click(within(await screen.findByLabelText('Matching tracks')).getByText('Artist - Title t1'))
  expect(await screen.findByText('0.920')).toBeTruthy()
  expect(screen.getByText('Warm-up', { selector: 'dd' })).toBeTruthy()
})

it('changes colour without rebuilding, and seeds only on request', async () => {
  render(LibraryMap)
  await screen.findByText(/3 of 4 library tracks/)
  await fireEvent.change(screen.getByLabelText('Colour by'), { target: { value: 'genre' } })
  expect(screen.getByText('Genre unknown')).toBeTruthy()
  expect(calls.some((c) => c.cmd === 'map_rebuild')).toBe(false)
  await fireEvent.input(screen.getByLabelText('Find a track'), { target: { value: 'title t2' } })
  await fireEvent.click(within(await screen.findByLabelText('Matching tracks')).getByText('Artist - Title t2'))
  expect(calls.some((c) => c.cmd === 'discovery_seeds_save' || c.cmd === 'playlist_add_tracks')).toBe(false)
  await fireEvent.click(screen.getByText('Use as discovery seeds'))
  await waitFor(() => expect(calls.find((c) => c.cmd === 'discovery_seeds_save')?.args).toEqual({
    seeds: [{ kind: 'artist', value: 'Existing' }, { kind: 'track', value: 'Artist - Title t2' }],
  }))
})

it('rebuilds with the library-chosen distance and lists tracks it cannot place', async () => {
  render(LibraryMap)
  await fireEvent.click(await screen.findByText('Rebuild map'))
  await waitFor(() => expect(calls.find((c) => c.cmd === 'map_rebuild')?.args).toEqual({
    params: { neighbours: 10, min_samples: 5, eps: null, epochs: 200, seed: 42 },
  }))
  const details = screen.getByText('1 library tracks are not on the map').closest('details')!
  details.open = true
  await fireEvent(details, new Event('toggle'))
  expect(await screen.findByText('Waiting for analysis.')).toBeTruthy()
})
