import { describe, expect, it } from 'vitest'
import { formatBytes, formatDuration, formatRelative, trackLabel } from './format'

describe('formatDuration', () => {
  it('formats minutes and seconds', () => {
    expect(formatDuration(0)).toBe('0:00')
    expect(formatDuration(61_000)).toBe('1:01')
    expect(formatDuration(3_723_000)).toBe('1:02:03')
  })

  it('shows a placeholder for unknown durations', () => {
    expect(formatDuration(null)).toBe('-:--')
    expect(formatDuration(-5)).toBe('-:--')
  })
})

describe('formatBytes', () => {
  it('picks a sensible unit', () => {
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(5 * 1024 ** 2)).toBe('5 MB')
    expect(formatBytes(10 * 1024 ** 3)).toBe('10.0 GB')
  })
})

describe('formatRelative', () => {
  it('describes past and future times', () => {
    expect(formatRelative(1000, 1000)).toBe('now')
    expect(formatRelative(0, 30_000)).toBe('30 s ago')
    expect(formatRelative(120_000, 0)).toBe('in 2 min')
  })
})

describe('trackLabel', () => {
  it('includes artist and mix when present', () => {
    expect(trackLabel({ artist: 'A', title: 'B', mix: 'Dub' })).toBe('A - B (Dub)')
    expect(trackLabel({ artist: null, title: null, mix: null })).toBe('Untitled')
  })
})
