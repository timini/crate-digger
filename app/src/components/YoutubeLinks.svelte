<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener'
  import { connections, type YoutubeLink, type YoutubeStatus } from '../lib/api'

  let {
    trackId,
    links,
    status,
    onchanged,
  }: { trackId: string; links: YoutubeLink[]; status: YoutubeStatus | null | undefined; onchanged: () => void } =
    $props()

  let url = $state('')
  let error = $state('')
  let busy = $state(false)

  const statusText: Record<YoutubeStatus['status'], string> = {
    queued: 'Looking up a YouTube reference.',
    found: '',
    uncertain: 'No confident match. Pick one below or paste the right link.',
    none: 'No matching video found.',
    waiting: 'Waiting',
    failed: 'Lookup failed',
  }

  async function run(action: () => Promise<void>) {
    busy = true
    error = ''
    try {
      await action()
      onchanged()
    } catch (e) {
      error = String(e)
    } finally {
      busy = false
    }
  }
</script>

{#if status && statusText[status.status]}
  <p class="muted small">
    {statusText[status.status]}{#if status.detail && (status.status === 'waiting' || status.status === 'failed')}: {status.detail}{/if}
  </p>
{:else if !status && !links.length}
  <p class="muted">No YouTube reference yet.</p>
{/if}
{#each links as y (y.video_id)}
  <div class="link-row">
    <button class="link" onclick={() => openUrl(y.url)}>{y.title ?? y.url}</button>
    <span class="muted small">
      {y.channel ?? ''} · {y.user_corrected && y.preferred ? 'your choice' : `${Math.round(y.confidence * 100)}%`}{y.preferred ? ' · preferred' : ''}
    </span>
    {#if !y.preferred}
      <button disabled={busy} onclick={() => run(() => connections.youtubePrefer(trackId, y.video_id))}>Use this</button>
    {/if}
    <button disabled={busy} onclick={() => run(() => connections.youtubeReject(trackId, y.video_id))}>Wrong</button>
  </div>
{/each}
<form onsubmit={(e) => { e.preventDefault(); void run(async () => { await connections.youtubeSet(trackId, url); url = '' }) }}>
  <input aria-label="Correct YouTube link" bind:value={url} placeholder="Paste the right YouTube link" />
  <button disabled={busy || !url.trim()}>Save link</button>
  <button type="button" disabled={busy} onclick={() => run(() => connections.youtubeRefresh(trackId))}>Look up again</button>
</form>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<style>
  .link-row { display: flex; flex-wrap: wrap; gap: 6px; align-items: baseline; margin: 4px 0; }
  form { display: flex; gap: 6px; margin-top: 6px; }
  input { flex: 1; min-width: 0; }
</style>
