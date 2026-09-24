import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'

const calls: { cmd: string; args: unknown }[] = []
const current = { artist: 'kerri chandler', title: 'track 01', mix: null, label: null, release: null, track_number: null, year: null, genre: null, tempo: null, musical_key: null }
const suggestion = (id: string, title: string, score: number) => ({
  id, track_id: 't1', current, score,
  found: { score, duration_ms: 392000, fields: [['artist', 'Kerri Chandler'], ['title', title], ['release', 'Rain EP']], discogs_fields: [] },
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  if (cmd === 'metadata_suggestions') return [suggestion('s1', 'Rain', 0.85), suggestion('s2', 'Rainfall', 0.8)]
  if (cmd === 'metadata_identify_all') return 12
  return null
}) }))
import MetadataCheck from './MetadataCheck.svelte'
afterEach(() => { cleanup(); calls.length = 0 })

it('queues identification and lets the user pick a match or keep the track as it is', async () => {
  const onchange = vi.fn()
  render(MetadataCheck, { onchange })
  await fireEvent.click(screen.getByText('Identify all'))
  expect(await screen.findByText(/Looking up 12 tracks/)).toBeTruthy()
  expect(screen.getByText('1 track needs a metadata check')).toBeTruthy()
  expect(screen.getByText(/Kerri Chandler - Rain$/)).toBeTruthy()
  await fireEvent.click(screen.getAllByText('Use this')[0])
  await waitFor(() => expect(calls.find(c => c.cmd === 'metadata_accept')?.args).toEqual({ id: 's1' }))
  await fireEvent.click(screen.getByText('Keep as it is'))
  await waitFor(() => expect(calls.find(c => c.cmd === 'metadata_dismiss')?.args).toEqual({ trackId: 't1' }))
  expect(onchange).toHaveBeenCalled()
})
