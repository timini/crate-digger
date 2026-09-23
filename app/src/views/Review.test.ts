import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Record every Tauri command the view sends.
const calls: { cmd: string; args: unknown }[] = []
const card = {
  candidate_id: 'c1',
  track_id: 't1',
  meta: { artist: 'Mock Unit', title: 'Deterministic', mix: 'Dub', label: 'Seeded Sounds', release: null, track_number: null, year: null, genre: null, tempo: 124, musical_key: '8A' },
  file: { id: 'f1', track_id: 't1', path: '/s/a.wav', origin: 'staged', size_bytes: 1, duration_ms: 20000, format: 'pcm_s16le', sample_rate: 44100, channels: 2, bitrate_kbps: null, availability: 'available', availability_reason: null, is_primary: true },
  reasons: ['Released on Seeded Sounds'],
  evidence: [{ source_kind: 'demo', source_url: 'https://example.invalid/x', supplied_text_id: null, retrieved_at: 0, excerpt: 'Mock Unit - Deterministic', confidence: 0.8 }],
  youtube: [],
  confidence: 0.8,
  verified: true,
  kept: false,
  playlists: [],
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args: unknown) => {
    calls.push({ cmd, args })
    switch (cmd) {
      case 'review_next':
        return [card]
      case 'review_stats':
        return { ready: 1, in_progress: 0, skipped: 0, needs_review: 0, failed: 0, reviewed: 0 }
      case 'demo_discovery_get':
        return true
      case 'playlists_list':
        return []
      case 'player_status':
        return { track_id: 't1', state: 'playing', position_ms: 0, duration_ms: 20000, volume: 1 }
      case 'review_undo':
        return { track_id: 't1', kind: 'star2', effective: null }
      default:
        return null
    }
  }),
}))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))

import Review from './Review.svelte'

async function press(key: string) {
  await fireEvent.keyDown(window, { key })
}

function sent(cmd: string) {
  return calls.filter((c) => c.cmd === cmd)
}

describe('Review keyboard control', () => {
  afterEach(cleanup)

  beforeEach(async () => {
    calls.length = 0
    render(Review)
    await screen.findByRole('heading', { name: /Deterministic/ })
  })

  it('shows the card with evidence and reasons', () => {
    expect(screen.getByText('Mock Unit')).toBeTruthy()
    expect(screen.getByText('Released on Seeded Sounds')).toBeTruthy()
    expect(screen.getByText('https://example.invalid/x')).toBeTruthy()
    expect(sent('player_play_track')[0].args).toMatchObject({ trackId: 't1' })
  })

  it.each([
    ['0', 'thumbs_down'],
    ['1', 'star1'],
    ['2', 'star2'],
    ['3', 'star3'],
  ])('key %s rates %s', async (key, kind) => {
    await press(key)
    await waitFor(() => expect(sent('review_rate')).toHaveLength(1))
    expect(sent('review_rate')[0].args).toEqual({ trackId: 't1', kind })
  })

  it('S skips, Z undoes and K keeps', async () => {
    await press('s')
    await waitFor(() => expect(sent('review_skip')).toHaveLength(1))
    await press('z')
    await waitFor(() => expect(sent('review_undo')).toHaveLength(1))
    await press('k')
    await waitFor(() => expect(sent('review_keep')).toHaveLength(1))
    expect(sent('review_keep')[0].args).toEqual({ trackId: 't1', keep: true })
  })

  it('P focuses the add-to-playlist control', async () => {
    await press('p')
    expect(document.activeElement?.getAttribute('aria-label')).toBe('Add to playlist')
  })

  it('arrow keys seek', async () => {
    await press('ArrowRight')
    await waitFor(() => expect(sent('player_seek')).toHaveLength(1))
  })

  it('rates a card only once even if pressed twice quickly', async () => {
    await press('2')
    await press('2')
    await waitFor(() => expect(sent('review_next').length).toBeGreaterThan(1))
    expect(sent('review_rate')).toHaveLength(1)
  })

  it('ignores keys typed into inputs', async () => {
    const input = document.createElement('input')
    document.body.appendChild(input)
    await fireEvent.keyDown(input, { key: '3' })
    expect(sent('review_rate')).toHaveLength(0)
  })
})
