import { cleanup, render, screen } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => ({
  buffer: { ready: 4, in_progress: 12, target: 50, below: 30 },
  holds: [
    { code: 'auth', message: 'slskd needs attention: Soulseek is not running.' },
    { code: 'daily_limit', message: 'Daily limit of 100 reached.' },
    { code: 'no_candidates', message: 'The last discovery run found nothing new.' },
  ],
  availability: { samples: 10, any_ready: 0.9, above_threshold: 0.4 },
})) }))
import QueueHealth from './QueueHealth.svelte'
afterEach(cleanup)

it('shows each reason the queue is short separately, and availability', async () => {
  render(QueueHealth)
  expect(await screen.findByText('4 of 50 ready, 12 on the way.')).toBeTruthy()
  expect(screen.getByText(/Soulseek is not running/).dataset.code).toBe('auth')
  expect(screen.getByText(/Daily limit/).dataset.code).toBe('daily_limit')
  expect(screen.getByText(/found nothing new/).dataset.code).toBe('no_candidates')
  expect(screen.getByText(/ready 90% of the time/)).toBeTruthy()
})
