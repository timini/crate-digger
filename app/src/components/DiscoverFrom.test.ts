import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'

const calls: { cmd: string; args: unknown }[] = []
const runs = [
  { source: 'pages', input: 'page https://example.com/list', started_at: 2, finished_at: 2, outcome: 'failed', created: 0, already_known: 0, unverified: 0, detail: "service unavailable: The site's robots.txt could not be read" },
  { source: 'live', input: 'seeds and ratings', started_at: 1, finished_at: 1, outcome: 'empty', created: 0, already_known: 0, unverified: 0, detail: null },
  { source: 'live', input: 'seeds and ratings', started_at: 0, finished_at: 0, outcome: 'found', created: 4, already_known: 0, unverified: 1, detail: null },
]
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  return cmd === 'discovery_runs' ? runs : null
}) }))
import DiscoverFrom from './DiscoverFrom.svelte'
afterEach(() => { cleanup(); calls.length = 0 })

it('shows failed runs apart from runs that found nothing', async () => {
  render(DiscoverFrom)
  expect(await screen.findByText(/Failed: service unavailable/)).toBeTruthy()
  expect(screen.getByText('No tracks found')).toBeTruthy()
  expect(screen.getByText('Found 4 new, 1 unverified')).toBeTruthy()
})

it('sends a page address and pasted text', async () => {
  render(DiscoverFrom)
  await fireEvent.input(screen.getByLabelText(/Public tracklist/), { target: { value: 'https://example.com/list' } })
  await fireEvent.click(screen.getByText('Read page'))
  await waitFor(() => expect(calls.find(c => c.cmd === 'discovery_page')?.args).toEqual({ url: 'https://example.com/list' }))
  await fireEvent.input(screen.getByLabelText(/paste text/), { target: { value: 'Alpha - One' } })
  await fireEvent.click(screen.getByText('Use text'))
  await waitFor(() => expect(calls.find(c => c.cmd === 'discovery_paste')?.args).toEqual({ text: 'Alpha - One', label: null }))
})
