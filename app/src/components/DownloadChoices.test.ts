import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'

const calls: { cmd: string; args: unknown }[] = []
const option = (id: string, file: string, acceptable: boolean, notes: string[]) => ({
  result: { result_id: id, filename: `@@u\\Music\\${file}`, size_bytes: 1, duration_ms: 300000, format: 'flac', bitrate_kbps: null, username: 'dj one', free_slot: true, queue_length: 0 },
  quality: 'lossless', mix: acceptable ? 'compatible' : 'uncertain', acceptable, notes,
})
let pending = [{
  candidate_id: 'c1', track_id: 't1',
  meta: { artist: 'Alpha Unit', title: 'First Light', mix: null, label: null, release: null, track_number: null, year: null, genre: null, tempo: null, musical_key: null },
  why: 'Unattended downloads are off. The recommended copy meets the automatic rule.',
  outcome: { ranked: [option('r1', 'Alpha Unit - First Light.flac', true, []), option('r2', 'Alpha Unit - First Light (Extended Mix).flac', false, ['the mix may differ'])], decision: { kind: 'choose', recommended: 0, why: '' } },
  created_at: 0,
}]
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  if (cmd === 'download_choices') return pending
  return null
}) }))
import DownloadChoices from './DownloadChoices.svelte'
afterEach(() => { cleanup(); calls.length = 0 })

it('shows ranked options with the recommendation and reasons', async () => {
  render(DownloadChoices)
  expect(await screen.findByText('1 download needs your choice')).toBeTruthy()
  expect(screen.getByText('Recommended')).toBeTruthy()
  expect(screen.getByText('the mix may differ')).toBeTruthy()
})

it('downloads the chosen option or declines all', async () => {
  render(DownloadChoices)
  const buttons = await screen.findAllByText('Download')
  await fireEvent.click(buttons[1])
  await waitFor(() => expect(calls.find(c => c.cmd === 'download_choose')?.args).toEqual({ candidateId: 'c1', resultId: 'r2' }))
  await fireEvent.click(screen.getByText('None of these'))
  await waitFor(() => expect(calls.find(c => c.cmd === 'download_decline')?.args).toEqual({ candidateId: 'c1' }))
})
