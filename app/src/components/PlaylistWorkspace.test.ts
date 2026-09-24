import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'
const calls: { cmd: string; args: unknown }[] = []
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  if (cmd === 'playlist_workspace') return { seeds: [{ kind: 'label', value: 'Echocord' }], ready: 3, runs: [{ source: 'live', input: 'seeds and ratings', started_at: 0, finished_at: Date.now(), outcome: 'found', created: 4, already_known: 1, unverified: 0, detail: null, playlist_id: 'p1' }] }
  return null
}) }))
import PlaylistWorkspace from './PlaylistWorkspace.svelte'
import { nav } from '../lib/nav.svelte'

const playlist = { id: 'p1', name: 'Warm-up', brief: 'Dubby', discovery: false, track_count: 2, duration_ms: 0, created_at: 0, updated_at: 0 }
afterEach(() => { cleanup(); calls.length = 0 })

it('saves the brief and seeds only when asked', async () => {
  render(PlaylistWorkspace, { playlist })
  expect(await screen.findByText('Echocord (label)')).toBeTruthy()
  await fireEvent.input(screen.getByLabelText('Brief'), { target: { value: 'Dubby, spacious' } })
  expect(calls.some((c) => c.cmd === 'playlist_brief_set')).toBe(false)
  await fireEvent.click(screen.getByText('Save brief'))
  await waitFor(() => expect(calls.find((c) => c.cmd === 'playlist_brief_set')?.args).toEqual({ id: 'p1', brief: 'Dubby, spacious' }))

  await fireEvent.click(screen.getByText('Edit seeds'))
  await fireEvent.click(screen.getByText('Add seed'))
  const values = screen.getAllByLabelText('Seed value')
  await fireEvent.input(values[1], { target: { value: 'Rhythm & Sound' } })
  await fireEvent.click(screen.getByText('Save seeds'))
  await waitFor(() => expect(calls.find((c) => c.cmd === 'playlist_seeds_save')?.args).toEqual({
    id: 'p1', seeds: [{ kind: 'label', value: 'Echocord' }, { kind: 'artist', value: 'Rhythm & Sound' }],
  }))
})

it("turns discovery on and opens the playlist's suggestions", async () => {
  render(PlaylistWorkspace, { playlist })
  await fireEvent.click(await screen.findByLabelText(/Look for tracks for this playlist/))
  await waitFor(() => expect(calls.find((c) => c.cmd === 'playlist_discovery_set')?.args).toEqual({ id: 'p1', on: true }))
  expect(screen.getByText(/4 new, 1 already known/)).toBeTruthy()
  await fireEvent.click(screen.getByText('Review 3 suggestions'))
  expect(nav.view).toBe('review')
  expect(nav.reviewPlaylist).toBe('p1')
})
