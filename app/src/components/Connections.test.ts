import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'
const calls: { cmd: string; args: unknown }[] = []
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  if (cmd === 'connections_get') return { llm_provider: 'openai_compatible', llm_endpoint: 'http://localhost:11434/v1', llm_model: '', slskd_endpoint: 'http://localhost:5030', external_slskd: false, enabled: false }
  if (cmd === 'discovery_seeds') return []
  return null
}) }))
import Connections from './Connections.svelte'
afterEach(() => { cleanup(); calls.length = 0 })
it('sends credentials only to the keychain command and clears the field', async () => {
  render(Connections)
  const input = await screen.findByLabelText('Discogs personal token')
  await fireEvent.input(input, { target: { value: 'fixture-secret' } })
  const row = input.closest('.credential')!
  await fireEvent.click(row.querySelector('button')!)
  await waitFor(() => expect((input as HTMLInputElement).value).toBe(''))
  expect(calls.find(c => c.cmd === 'credential_set')?.args).toEqual({ key: 'discogs', value: 'fixture-secret' })
  await fireEvent.click(screen.getByText('Save connections'))
  await waitFor(() => expect(calls.some(c => c.cmd === 'connections_save')).toBe(true))
  expect(JSON.stringify(calls.find(c => c.cmd === 'connections_save'))).not.toContain('fixture-secret')
})
it('saves editable seeds without requiring a connection', async () => {
  render(Connections)
  await fireEvent.click(await screen.findByText('Add seed'))
  await fireEvent.input(screen.getByLabelText('Seed value'), { target: { value: 'Fixture artist' } })
  await fireEvent.click(screen.getByText('Save seeds'))
  await waitFor(() => expect(calls.find(c => c.cmd === 'discovery_seeds_save')?.args).toEqual({ seeds: [{ kind: 'artist', value: 'Fixture artist' }] }))
})
