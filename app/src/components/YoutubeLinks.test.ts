import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'

const calls: { cmd: string; args: unknown }[] = []
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => { calls.push({ cmd, args }); return null }) }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))
import YoutubeLinks from './YoutubeLinks.svelte'
afterEach(() => { cleanup(); calls.length = 0 })

const link = (id: string, preferred: boolean, confidence: number) => ({
  video_id: id, url: `https://www.youtube.com/watch?v=${id}`, title: `Video ${id}`, channel: 'Channel',
  duration_ms: null, confidence, preferred, user_corrected: false,
})

it('shows why a lookup is waiting', () => {
  render(YoutubeLinks, { trackId: 't', links: [], status: { status: 'waiting', detail: 'authentication failed: Add a YouTube Data API key in Settings.', updated_at: 0 }, onchanged: () => {} })
  expect(screen.getByText(/Waiting: .*YouTube Data API key/)).toBeTruthy()
})

it('lets the user choose, reject or replace a link', async () => {
  const onchanged = vi.fn()
  render(YoutubeLinks, { trackId: 't', links: [link('a', false, 0.6), link('b', false, 0.5)], status: { status: 'uncertain', detail: null, updated_at: 0 }, onchanged })
  expect(screen.getByText(/No confident match/)).toBeTruthy()
  await fireEvent.click(screen.getAllByText('Use this')[0])
  await waitFor(() => expect(calls.find(c => c.cmd === 'youtube_prefer')?.args).toEqual({ trackId: 't', videoId: 'a' }))
  await fireEvent.click(screen.getAllByText('Wrong')[1])
  await waitFor(() => expect(calls.find(c => c.cmd === 'youtube_reject')?.args).toEqual({ trackId: 't', videoId: 'b' }))
  await fireEvent.input(screen.getByLabelText('Correct YouTube link'), { target: { value: 'https://youtu.be/xxxxxxxxxxx' } })
  await fireEvent.click(screen.getByText('Save link'))
  await waitFor(() => expect(calls.find(c => c.cmd === 'youtube_set')?.args).toEqual({ trackId: 't', url: 'https://youtu.be/xxxxxxxxxxx' }))
  expect(onchanged).toHaveBeenCalledTimes(3)
})
