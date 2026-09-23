<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { api, type Activity, type JobState } from '../lib/api'
  import { formatBytes, formatRelative } from '../lib/format'

  const filters: { label: string; states: JobState[] }[] = [
    { label: 'Active', states: ['queued', 'running', 'paused', 'blocked'] },
    { label: 'Failed', states: ['failed'] },
    { label: 'All', states: [] },
  ]

  let filter = $state(0)
  let activity: Activity | null = $state(null)
  let error: string | null = $state(null)
  let timer: ReturnType<typeof setInterval> | undefined

  async function load() {
    try {
      activity = await api.activity(filters[filter].states)
      error = null
    } catch (e) {
      error = String(e)
    }
  }

  async function act(fn: () => Promise<unknown>) {
    try {
      await fn()
    } catch (e) {
      error = String(e)
    }
    await load()
  }

  function total(state: JobState): number {
    return activity?.counts.filter((c) => c.state === state).reduce((n, c) => n + c.count, 0) ?? 0
  }

  onMount(() => {
    load()
    timer = setInterval(load, 2000)
  })
  onDestroy(() => clearInterval(timer))
</script>

<section>
  <header class="bar">
    <h2>Activity</h2>
    <div class="actions">
      <button onclick={() => act(api.pauseAll)}>Pause all</button>
      <button onclick={() => act(api.resumeAll)}>Resume</button>
    </div>
  </header>

  {#if error}<p class="error">{error}</p>{/if}

  {#if activity}
    <div class="summary">
      <div><strong>{total('running')}</strong> running</div>
      <div><strong>{total('queued')}</strong> queued</div>
      <div><strong>{total('paused') + total('blocked')}</strong> waiting</div>
      <div><strong>{total('failed')}</strong> failed</div>
      <div>
        Temporary audio {formatBytes(activity.scheduler.staging_used_bytes)} of
        {formatBytes(activity.scheduler.staging_budget_bytes)}
      </div>
      {#each activity.scheduler.daily as d}
        <div>{d.used} of {d.limit} {d.kind} jobs today</div>
      {/each}
    </div>

    {#each activity.connectors.filter((c) => c.status !== 'ok') as c}
      <p class="warn">{c.connector}: {c.status === 'auth_failed' ? 'sign-in failed' : 'unavailable'}. {c.reason}</p>
    {/each}

    <div class="tabs" role="tablist">
      {#each filters as f, i}
        <button role="tab" aria-selected={filter === i} class:active={filter === i} onclick={() => ((filter = i), load())}>
          {f.label}
        </button>
      {/each}
    </div>

    {#if activity.jobs.length === 0}
      <p class="muted">Nothing here.</p>
    {:else}
      <table>
        <thead>
          <tr><th>Job</th><th>State</th><th>Reason</th><th>Updated</th><th></th></tr>
        </thead>
        <tbody>
          {#each activity.jobs as job (job.id)}
            <tr>
              <td>{job.kind}{job.connector ? ` (${job.connector})` : ''}</td>
              <td><span class="state {job.state}">{job.state}</span></td>
              <td class="reason">{job.reason ?? ''}</td>
              <td class="muted">{formatRelative(job.updated_at)}</td>
              <td class="row-actions">
                {#if job.state === 'failed' || job.state === 'cancelled'}
                  <button onclick={() => act(() => api.retryJob(job.id))}>Retry</button>
                {/if}
                {#if job.state !== 'done' && job.state !== 'cancelled'}
                  <button onclick={() => act(() => api.cancelJob(job.id))}>Cancel</button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/if}
</section>

<style>
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .summary {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 24px;
    padding: 12px 16px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    margin: 12px 0;
  }
  .tabs {
    display: flex;
    gap: 4px;
    margin: 12px 0;
  }
  .tabs .active {
    border-color: var(--accent);
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th,
  td {
    text-align: left;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
    vertical-align: top;
  }
  .reason {
    max-width: 480px;
  }
  .row-actions {
    white-space: nowrap;
    display: flex;
    gap: 4px;
  }
  .state {
    font-size: 12px;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--panel-2);
  }
  .state.running {
    color: var(--ok);
  }
  .state.failed {
    color: var(--danger);
  }
  .state.paused,
  .state.blocked {
    color: var(--accent);
  }
  .warn {
    color: var(--accent);
  }
</style>
