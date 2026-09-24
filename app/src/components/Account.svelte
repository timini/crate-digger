<script lang="ts">
  import { onMount } from 'svelte'
  import { central, type BackupInfo, type CentralStatus, type RestoreSummary } from '../lib/api'
  import { formatBytes } from '../lib/format'

  let status: CentralStatus | null = $state(null)
  let backups: BackupInfo[] = $state([])
  let busy = $state(false)
  let message = $state('')
  let error = $state('')
  let confirming: string | null = $state(null)

  async function load() {
    status = await central.status()
    backups = status.signed_in ? await central.backups().catch(() => []) : []
  }

  async function run(action: () => Promise<string | void>) {
    busy = true; error = ''; message = ''
    try { message = (await action()) ?? '' } catch (e) { error = String(e) }
    finally { busy = false }
    await load().catch((e) => (error = String(e)))
  }

  function restored(s: RestoreSummary) {
    const parts = [`${s.matched} tracks found`, `${s.ratings} ratings`, `${s.playlists} playlists`]
    if (s.to_relink) parts.push(`${s.to_relink} tracks need their files found: use Find moved files in the Library`)
    return `Restored: ${parts.join(', ')}.`
  }

  onMount(() => { void load().catch((e) => (error = String(e))) })
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p role="status">{message}</p>{/if}
{#if status}
  {#if !status.configured}
    <p class="muted">Add the shared catalogue address and a Google client id under Connections to sign in.</p>
  {:else if !status.signed_in}
    <p>Signing in lets you back up your ratings and playlists, and optionally share what you identify.</p>
    <button disabled={busy} onclick={() => run(async () => {
      const email = await central.signIn()
      return email ? `Signed in as ${email}.` : 'Signed in.'
    })}>{busy ? 'Waiting for the browser...' : 'Sign in with Google'}</button>
  {:else}
    <p>Signed in{status.email ? ` as ${status.email}` : ''}.
      <button disabled={busy} onclick={() => run(async () => { await central.signOut(); return 'Signed out.' })}>Sign out</button></p>

    <label class="check">
      <input type="checkbox" checked={status.sharing} disabled={busy}
        onchange={(e) => { const on = e.currentTarget.checked; void run(async () => {
          const n = await central.setSharing(on)
          return on ? `Sharing on. ${n} identified tracks queued.` : 'Sharing off.'
        }) }} />
      Share identified tracks with the catalogue: their MusicBrainz, ISRC and Discogs ids, track details and audio features. Never ratings, file names, paths or audio.
    </label>
    {#if status.sharing}
      <p class="muted small">{status.outbox.pending} waiting, {status.outbox.acked} shared, {status.outbox.rejected} refused.</p>
    {/if}

    <h4>Backups</h4>
    <p class="muted small">Ratings, keeps, seeds, playlists and track details. No audio and no credentials.</p>
    <button disabled={busy} onclick={() => run(async () => { await central.backupNow(); return 'Backed up.' })}>Back up now</button>
    {#if backups.length === 0}
      <p class="muted">No backups yet.</p>
    {:else}
      <ul class="backups">
        {#each backups as b (b.id)}
          <li>
            <span>{new Date(b.created_at_ms).toLocaleString()} · {formatBytes(b.size_bytes)}</span>
            {#if confirming === b.id}
              <span>Restore adds to this library and changes nothing else.</span>
              <button disabled={busy} onclick={() => { confirming = null; void run(async () => restored(await central.restore(b.id))) }}>Restore</button>
              <button onclick={() => (confirming = null)}>Cancel</button>
            {:else}
              <button disabled={busy} onclick={() => (confirming = b.id)}>Restore...</button>
              <button disabled={busy} onclick={() => run(async () => { await central.deleteBackup(b.id); return 'Backup deleted.' })}>Delete</button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
{/if}

<style>
  .check { display: flex; gap: 8px; align-items: flex-start; margin: 12px 0; }
  .backups { list-style: none; padding: 0; display: grid; gap: 6px; }
  .backups li { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .small { font-size: 12px; }
</style>
