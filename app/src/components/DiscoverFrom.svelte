<script lang="ts">
  import { onMount } from 'svelte'
  import { connections, type SourceRun } from '../lib/api'

  let url = $state('')
  let text = $state('')
  let runs: SourceRun[] = $state([])
  let message = $state('')
  let error = $state('')

  async function refresh() {
    try {
      runs = await connections.runs()
    } catch (e) {
      error = String(e)
    }
  }
  onMount(refresh)

  async function submit(action: () => Promise<void>, done: string) {
    error = ''
    message = ''
    try {
      await action()
      message = done
      await refresh()
    } catch (e) {
      error = String(e)
    }
  }

  function describe(r: SourceRun): string {
    if (r.outcome === 'failed') return `Failed: ${r.detail ?? 'no reason given'}`
    if (r.outcome === 'empty') return r.already_known ? `Nothing new (${r.already_known} already known)` : 'No tracks found'
    const unverified = r.unverified ? `, ${r.unverified} unverified` : ''
    return `Found ${r.created} new${unverified}`
  }
</script>

<div class="discover">
  <form onsubmit={(e) => { e.preventDefault(); void submit(async () => { await connections.fromPage(url); url = '' }, 'Reading the page.') }}>
    <label>Public tracklist or recommendation page
      <input type="url" bind:value={url} placeholder="https://" />
    </label>
    <button disabled={!url.trim()}>Read page</button>
  </form>
  <form onsubmit={(e) => { e.preventDefault(); void submit(async () => { await connections.fromText(text, null); text = '' }, 'Reading the pasted text.') }}>
    <label>Or paste text, one track per line works best
      <textarea rows="4" bind:value={text} placeholder="Artist - Title (Mix)"></textarea>
    </label>
    <button disabled={!text.trim()}>Use text</button>
  </form>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
  {#if runs.length}
    <h4>Recent discovery</h4>
    <ul>
      {#each runs as r}
        <li class:failed={r.outcome === 'failed'}>
          <span class="muted small">{new Date(r.finished_at).toLocaleString()} · {r.input}</span>
          {describe(r)}
        </li>
      {/each}
    </ul>
    <button class="link" onclick={refresh}>Refresh</button>
  {/if}
</div>

<style>
  .discover { display: grid; gap: 10px; text-align: left; max-width: 560px; }
  form { display: flex; gap: 8px; align-items: flex-end; }
  label { display: grid; gap: 4px; flex: 1; }
  ul { margin: 0; padding-left: 18px; }
  li { margin: 4px 0; }
  li span { display: block; }
  .failed { color: var(--danger, #c44); }
</style>
