<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { openUrl } from '@tauri-apps/plugin-opener'
  import { connections, type SoulseekStatus } from '../lib/api'

  let status: SoulseekStatus | null = $state(null)
  let unattended = $state(false)
  let error = $state('')
  let timer: ReturnType<typeof setInterval> | undefined

  async function refresh() {
    try {
      status = await connections.soulseekStatus()
    } catch (e) {
      error = String(e)
    }
  }

  onMount(async () => {
    await refresh()
    unattended = await connections.unattendedGet()
    timer = setInterval(refresh, 1500)
  })
  onDestroy(() => clearInterval(timer))

  async function setup() {
    error = ''
    try {
      await connections.soulseekSetup()
    } catch (e) {
      error = String(e)
    }
    await refresh()
  }

  async function setUnattended(on: boolean) {
    await connections.unattendedSet(on)
    unattended = on
  }

  const stateText: Record<SoulseekStatus['state']['state'], string> = {
    not_installed: 'Not set up yet.',
    stopped: 'Stopped.',
    starting: 'Starting and signing in.',
    running: 'Running and signed in.',
    signed_out: 'Running, not signed in',
    failed: 'Stopped with an error',
  }
  const mb = (b: number) => (b / 1_000_000).toFixed(0)
</script>

{#if status && !status.external}
  <div class="soulseek">
    <p>
      <strong>Soulseek:</strong>
      {stateText[status.state.state]}{#if status.state.detail}: {status.state.detail}{/if}
    </p>
    {#if status.busy && !status.installed && status.download_size}
      <progress max={status.download_size} value={status.downloaded_bytes}></progress>
      <span class="muted small">{mb(status.downloaded_bytes)} of {mb(status.download_size)} MB</span>
    {:else if status.state.state !== 'running' && status.state.state !== 'starting'}
      <button disabled={status.busy} onclick={setup}>{status.installed ? 'Start Soulseek' : 'Set up Soulseek'}</button>
    {/if}
    <p class="muted small">
      Save your Soulseek username and password below first. Crate Digger runs its own copy of slskd {status.version}
      ({status.licence}, <button class="link" onclick={() => openUrl(status!.source)}>source</button>) as a separate program,
      downloaded once{status.download_size ? ` (${mb(status.download_size)} MB)` : ''} and checked against a pinned checksum.
      Nothing from your library is shared.
    </p>
  </div>
{/if}
<label class="check">
  <input type="checkbox" checked={unattended} onchange={(e) => setUnattended(e.currentTarget.checked)} />
  Download automatically when a copy meets the matching rule
</label>
<p class="muted small">
  Off: you choose every download from a ranked list. The rule and its test cases are described in
  docs/acquisition-calibration.md. Ambiguous matches always ask.
</p>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<style>
  .soulseek { display: grid; gap: 6px; }
  progress { width: 100%; }
  .check { display: flex; gap: 8px; align-items: center; }
</style>
