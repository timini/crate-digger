<script lang="ts">
  import { save } from '@tauri-apps/plugin-dialog'
  import { pilot, type PilotReport } from '../lib/api'

  let report: PilotReport | null = $state(null)
  let error = $state('')
  let message = $state('')

  const pct = (x: number | null | undefined) => (x === null || x === undefined ? 'not enough data' : `${(x * 100).toFixed(0)}%`)
  const num = (x: number | null | undefined) => (x === null || x === undefined ? 'not enough data' : x.toFixed(3))

  async function show() {
    error = ''
    try { report = await pilot.report() } catch (e) { error = String(e) }
  }

  async function saveReport() {
    error = ''; message = ''
    const path = await save({ title: 'Save the pilot report', defaultPath: 'crate-digger-pilot-report.json', filters: [{ name: 'JSON', extensions: ['json'] }] })
    if (!path) return
    try { await pilot.save(path); message = 'Report saved.' } catch (e) { error = String(e) }
  }
</script>

<p class="muted">For DJs in the pilot (see the evaluation protocol). The report holds counts and rates only: no track names, file paths or seeds. It is never sent anywhere; you choose whether to share the file.</p>
<div class="row">
  <button onclick={show}>Show report</button>
  <button onclick={saveReport}>Save report...</button>
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p role="status">{message}</p>{/if}
{#if report}
  <dl>
    <dt>Tracks judged</dt><dd>{report.outcomes.judged}</dd>
    <dt>Two or three stars</dt><dd>{pct(report.outcomes.strong_positive_rate)}</dd>
    <dt>Kept</dt><dd>{pct(report.outcomes.keep_rate)}</dd>
    <dt>Wrong version</dt><dd>{pct(report.outcomes.wrong_version_rate)}</dd>
    <dt>Ranking by sources alone (AUC)</dt><dd>{num(report.ranking.auc_cultural)}</dd>
    <dt>Ranking with your taste (AUC)</dt><dd>{num(report.ranking.auc_combined)}</dd>
    <dt>Difference, 95% interval</dt>
    <dd>{report.ranking.auc_difference_interval ? report.ranking.auc_difference_interval.map((x) => x.toFixed(3)).join(' to ') : 'not enough data'}</dd>
    <dt>Ready tracks available</dt><dd>{pct(report.buffer.samples ? report.buffer.any_ready : null)}</dd>
  </dl>
  <p class="muted small">AUC is the chance that a two or three star track was ranked above another judged track, using only ratings made before it. 0.5 is no better than chance.</p>
{/if}

<style>
  .row { display: flex; gap: 8px; margin: 8px 0; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 4px 12px; }
  dd { margin: 0; }
  .small { font-size: 12px; }
</style>
