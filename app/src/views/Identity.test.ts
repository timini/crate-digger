import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, describe, expect, it, vi } from 'vitest'

const calls: { cmd: string; args: unknown }[] = []
const meta = (title: string, mix: string | null) => ({
  artist: 'Fixture Collective',
  title,
  mix,
  label: null,
  release: null,
  track_number: null,
  year: null,
  genre: null,
  tempo: null,
  musical_key: null,
})
let open = true

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args: unknown) => {
    calls.push({ cmd, args })
    if (cmd === 'identity_conflicts') {
      return open
        ? [
            {
              id: 'k1',
              a: { track_id: 'a', meta: meta('Song 1', 'Original Mix'), path: '/m/a.flac', duration_ms: 300000 },
              b: { track_id: 'b', meta: meta('Song 1', 'Extended Mix'), path: '/m/b.mp3', duration_ms: 301000 },
              reason: 'The audio matches but the tags name different mixes.',
              evidence: [{ kind: 'fingerprint_match', score: 0.95, coverage: 0.99, speed: 1.0 }],
              created_at: 0,
            },
          ]
        : []
    }
    if (cmd === 'identity_resolve') open = false
    return null
  }),
}))

import Identity from './Identity.svelte'

describe('Identity review', () => {
  afterEach(cleanup)

  it('shows both sides, the reason and the evidence', async () => {
    open = true
    render(Identity)
    await screen.findByText('The audio matches but the tags name different mixes.')
    expect(screen.getByText('Fixture Collective - Song 1 (Original Mix)')).toBeTruthy()
    expect(screen.getByText('Fixture Collective - Song 1 (Extended Mix)')).toBeTruthy()
    expect(screen.getByText(/95% alignment over 99% of the track/)).toBeTruthy()
  })

  it.each([
    ['Same recording (merge into A)', 'same_recording'],
    ['Different versions', 'different_version'],
    ['Not related', 'unrelated'],
  ])('%s resolves as %s', async (label, relation) => {
    open = true
    calls.length = 0
    render(Identity)
    await fireEvent.click(await screen.findByText(label))
    await waitFor(() => expect(calls.find((c) => c.cmd === 'identity_resolve')).toBeTruthy())
    expect(calls.find((c) => c.cmd === 'identity_resolve')!.args).toEqual({ conflictId: 'k1', relation })
    await screen.findByText('Nothing to review.')
  })
})
