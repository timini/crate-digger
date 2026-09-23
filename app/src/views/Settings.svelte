<script lang="ts">
  import { onMount } from 'svelte'
  import { open } from '@tauri-apps/plugin-dialog'
  import { api, type AppInfo, type AppSettings, type UserLimits } from '../lib/api'
  import Folders from '../components/Folders.svelte'

  let settings: AppSettings | null = $state(null)
  let info: AppInfo | null = $state(null)
  let limits: UserLimits | null = $state(null)
  let error: string | null = $state(null)
  let saved: string | null = $state(null)

  async function load() {
    settings = await api.settings()
    limits = { ...settings.limits }
    info = await api.appInfo()
  }

  async function run(fn: () => Promise<unknown>, msg: string) {
    try {
      await fn()
      error = null
      saved = msg
      setTimeout(() => (saved = null), 2500)
    } catch (e) {
      error = String(e)
    }
    await load()
  }

  async function chooseArchive() {
    const dir = await open({ directory: true, multiple: false, title: 'Where should kept tracks go?' })
    if (typeof dir === 'string') run(() => api.setArchiveDir(dir), 'Archive folder saved')
  }

  async function chooseStaging() {
    const dir = await open({ directory: true, multiple: false, title: 'Where should downloads wait?' })
    if (typeof dir === 'string') run(() => api.setStagingDir(dir), 'Staging folder saved; it applies after a restart')
  }

  const limitFields: { key: keyof UserLimits; label: string; step?: number }[] = [
    { key: 'ready_target', label: 'Ready-to-review target (tracks)' },
    { key: 'replenish_below', label: 'Find more when fewer than (tracks)' },
    { key: 'active_downloads', label: 'Active downloads' },
    { key: 'analysis_jobs', label: 'Concurrent analysis jobs' },
    { key: 'temp_budget_gb', label: 'Temporary audio budget (GB)', step: 0.5 },
    { key: 'daily_acquisitions', label: 'New downloads per day' },
    { key: 'source_refresh_hours', label: 'Refresh sources every (hours)' },
  ]

  onMount(load)
</script>

<section class="settings">
  <h2>Settings</h2>
  {#if error}<p class="error">{error}</p>{/if}
  {#if saved}<p class="saved" role="status">{saved}</p>{/if}

  <h3>Music folders</h3>
  <Folders />

  {#if settings}
    <h3>Archive</h3>
    <p>Kept tracks are moved to <code>{settings.archive_dir}</code>{settings.archive_dir_is_default ? ' (default)' : ''}.</p>
    <div class="row">
      <button onclick={chooseArchive}>Choose folder...</button>
      {#if !settings.archive_dir_is_default}
        <button onclick={() => run(() => api.setArchiveDir(null), 'Using the default archive folder')}>Use default</button>
      {/if}
    </div>
    <p class="muted small">Layout: Artist/Release/Track number - Title (Mix). Existing files are never overwritten.</p>
    <p>Downloads wait in <code>{settings.staging_dir}</code> until you keep or clear them.</p>
    <button onclick={chooseStaging}>Choose staging folder...</button>

    <h3>Background work</h3>
    {#if limits}
      <form class="limits" onsubmit={(e) => (e.preventDefault(), run(() => api.setLimits(limits!), 'Limits saved'))}>
        {#each limitFields as f (f.key)}
          <label>
            <span>{f.label}</span>
            <input type="number" min="0" step={f.step ?? 1} bind:value={limits[f.key]} />
          </label>
        {/each}
        <div class="row">
          <button type="submit" class="primary">Save limits</button>
          <button type="button" onclick={() => (limits = { ...settings!.limits })}>Undo changes</button>
        </div>
      </form>
    {/if}
    <label class="check">
      <input type="checkbox" checked={settings.close_to_tray} onchange={(e) => run(() => api.setCloseToTray(e.currentTarget.checked), 'Saved')} />
      Closing the window keeps background work running in the tray. Use Quit from the tray or app menu to stop it.
    </label>

    <h3>Connections</h3>
    <table class="integrations">
      <tbody>
        <tr><td>AI agent (API or local model)</td><td class="muted">Arrives with end-to-end discovery (#10)</td></tr>
        <tr><td>Soulseek via slskd</td><td class="muted">Arrives with end-to-end discovery (#12)</td></tr>
        <tr><td>Discovery seeds</td><td class="muted">Arrives with end-to-end discovery (#11)</td></tr>
        <tr><td>Shared catalogue and private backup</td><td class="muted">Arrives with the central service (#17 to #19)</td></tr>
      </tbody>
    </table>
    <p class="muted small">Library, playback, ratings and playlists work without any of these.</p>
    <label class="check">
      <input type="checkbox" checked={settings.demo_discovery} onchange={(e) => run(() => api.setDemoDiscovery(e.currentTarget.checked), 'Saved')} />
      Demo discovery: generate tone recordings so the review queue can be tried. Nothing is downloaded.
    </label>

    <h3>About</h3>
    <p class="muted small">
      Crate Digger {info?.version} · database schema {info?.schema_version}<br />
      Data folder: <code>{settings.data_dir}</code>
    </p>
  {/if}
</section>

<style>
  .settings {
    max-width: 760px;
  }
  h3 {
    margin-top: 28px;
    border-bottom: 1px solid var(--border);
    padding-bottom: 6px;
  }
  .row {
    display: flex;
    gap: 8px;
    margin: 8px 0;
  }
  .limits {
    display: grid;
    gap: 6px;
  }
  .limits label {
    display: grid;
    grid-template-columns: 1fr 120px;
    align-items: center;
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin: 12px 0;
  }
  .integrations td {
    padding: 4px 16px 4px 0;
  }
  .small {
    font-size: 12px;
  }
  code {
    word-break: break-all;
  }
  .saved {
    color: var(--ok);
  }
</style>
