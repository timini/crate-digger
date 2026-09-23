import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'

const calls: { cmd: string; args: unknown }[] = []
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn(async () => '/tmp/Set.xml') }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  if (cmd === 'playlist_export_check') return {
    name: 'Set',
    tracks: [{ position: 0, track_id: 'a', path: '/m/a.flac' }, { position: 1, track_id: 'b', path: '/s/b.flac' }],
    problems: [{ kind: 'temporary', position: 1, track: 'B' }, { kind: 'missing', position: 2, track: 'C' }],
  }
  if (cmd === 'playlist_export') return 2
  return null
}) }))
import ExportPlaylist from './ExportPlaylist.svelte'
afterEach(() => { cleanup(); calls.length = 0 })

it('reports problems before writing, then exports', async () => {
  render(ExportPlaylist, { id: 'p1' })
  await fireEvent.click(screen.getByText('Export'))
  expect(await screen.findByText(/1 track has no available audio/)).toBeTruthy()
  expect(screen.getByText(/still in temporary storage/)).toBeTruthy()
  expect(calls.some(c => c.cmd === 'playlist_export')).toBe(false)
  await fireEvent.click(screen.getByText('Save Rekordbox XML'))
  await waitFor(() => expect(calls.find(c => c.cmd === 'playlist_export')?.args).toEqual({ id: 'p1', format: 'rekordbox', path: '/tmp/Set.xml' }))
  expect(await screen.findByText('Exported 2 tracks to /tmp/Set.xml.')).toBeTruthy()
})
