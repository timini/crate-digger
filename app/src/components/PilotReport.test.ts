import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'
const calls: { cmd: string; args: unknown }[] = []
const saveDialog = vi.fn(async () => '/tmp/report.json')
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: () => saveDialog() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string, args: unknown) => {
  calls.push({ cmd, args })
  if (cmd === 'evaluation_report') return {
    report_version: 1, generated_at: 0, app_version: '0.1.0', model_id: 'm', first_judgement_at: null, last_judgement_at: null,
    ranking: { judgements: 40, strong_positives: 20, without_embedding: 0, auc_cultural: 0.5, auc_combined: 0.91, auc_difference_interval: [0.3, 0.5], top_fifth_strong_rate_cultural: 0.5, top_fifth_strong_rate_combined: 1 },
    outcomes: { judged: 40, thumbs_down: 20, one_star: 0, two_stars: 0, three_stars: 20, skipped: 0, strong_positive_rate: 0.5, keep_rate: null, wrong_version_rate: 0.025 },
    buffer: { samples: 0, any_ready: 0, above_threshold: 0 }, source_runs: {}, analysis: {},
    embedding_stability: { pairs: 0, mean_similarity: null, min_similarity: null }, not_recorded: [],
  }
  return null
}) }))
import PilotReport from './PilotReport.svelte'
afterEach(() => { cleanup(); calls.length = 0 })

it('shows the comparison and says when there is not enough data', async () => {
  render(PilotReport)
  expect(calls.length).toBe(0)
  await fireEvent.click(screen.getByText('Show report'))
  expect(await screen.findByText('0.910')).toBeTruthy()
  expect(screen.getByText('0.300 to 0.500')).toBeTruthy()
  expect(screen.getByText('3%')).toBeTruthy()
  expect(screen.getAllByText('not enough data').length).toBe(2)
})

it('saves only to the chosen file', async () => {
  render(PilotReport)
  await fireEvent.click(screen.getByText('Save report...'))
  await waitFor(() => expect(calls.find((c) => c.cmd === 'evaluation_save')?.args).toEqual({ path: '/tmp/report.json' }))
})
