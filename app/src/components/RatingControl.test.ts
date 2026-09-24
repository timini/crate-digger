import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'

const calls: { cmd: string; args: unknown }[] = []
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => { calls.push({ cmd, args }); return null }) }))
import RatingControl from './RatingControl.svelte'
afterEach(() => { cleanup(); calls.length = 0 })

it('sets, changes and clears a rating', async () => {
  const onchange = vi.fn()
  render(RatingControl, { trackId: 't1', rating: 'star1', onchange })
  expect(screen.getByLabelText('One star (1)').getAttribute('aria-pressed')).toBe('true')

  await fireEvent.click(screen.getByLabelText('Three stars (3)'))
  await waitFor(() => expect(calls.at(-1)?.args).toEqual({ trackId: 't1', kind: 'star3' }))
  expect(onchange).toHaveBeenLastCalledWith('star3')

  // Choosing the current rating again clears it.
  await fireEvent.click(screen.getByLabelText('One star (1)'))
  await waitFor(() => expect(calls.at(-1)?.args).toEqual({ trackId: 't1', kind: null }))
  expect(onchange).toHaveBeenLastCalledWith(null)
})
