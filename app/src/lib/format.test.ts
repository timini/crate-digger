import { describe, expect, it } from 'vitest'
import { formatDuration } from './format'

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
