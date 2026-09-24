import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'
const calls: { cmd: string; args: unknown }[] = []
let status = { configured: true, signed_in: false, email: null as string | null, sharing: false, outbox: { pending: 0, acked: 0, rejected: 0 } }
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  if (cmd === 'central_status') return status
  if (cmd === 'central_sign_in') { status = { ...status, signed_in: true, email: 'dj@example.com' }; return 'dj@example.com' }
  if (cmd === 'backups_list') return [{ id: 'b1', created_at_ms: 1_700_000_000_000, size_bytes: 2048, version: 1 }]
  if (cmd === 'sharing_set') return 3
  if (cmd === 'backup_restore') return { matched: 5, to_relink: 2, ratings: 4, playlists: 1, seeds: 0 }
  return null
}) }))
import Account from './Account.svelte'
afterEach(() => { cleanup(); calls.length = 0 })

it('explains what to set up before sign-in is possible', async () => {
  status = { ...status, configured: false, signed_in: false }
  render(Account)
  expect(await screen.findByText(/Add the shared catalogue address/)).toBeTruthy()
  expect(screen.queryByText('Sign in with Google')).toBeNull()
})

it('signs in, then shows backups and the sharing choice', async () => {
  status = { ...status, configured: true, signed_in: false, email: null }
  render(Account)
  await fireEvent.click(await screen.findByText('Sign in with Google'))
  expect(await screen.findByText('Signed in as dj@example.com.')).toBeTruthy()
  expect(await screen.findByText('Restore...')).toBeTruthy()
  const share = screen.getByLabelText(/Share identified tracks/) as HTMLInputElement
  expect(share.checked).toBe(false)
  await fireEvent.click(share)
  await waitFor(() => expect(calls.find(c => c.cmd === 'sharing_set')?.args).toEqual({ enabled: true }))
})

it('asks before restoring and reports tracks to relink', async () => {
  status = { ...status, configured: true, signed_in: true, email: 'dj@example.com' }
  render(Account)
  await fireEvent.click(await screen.findByText('Restore...'))
  expect(calls.some(c => c.cmd === 'backup_restore')).toBe(false)
  await fireEvent.click(screen.getByText('Restore'))
  expect(await screen.findByText(/2 tracks need their files found/)).toBeTruthy()
  expect(calls.find(c => c.cmd === 'backup_restore')?.args).toEqual({ id: 'b1' })
})
