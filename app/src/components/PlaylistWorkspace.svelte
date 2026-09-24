<script lang="ts">
  import { workspace, type Playlist, type Seed, type WorkspaceInfo } from '../lib/api'
  import { formatRelative } from '../lib/format'
  import { reviewFor } from '../lib/nav.svelte'

  let { playlist, onchange }: { playlist: Playlist; onchange?: () => void } = $props()

  let info: WorkspaceInfo | null = $state(null)
  let brief = $state('')
  let seeds: Seed[] = $state([])
  let error = $state('')
  let message = $state('')
  let editingSeeds = $state(false)

  async function load() {
    try {
      info = await workspace.get(playlist.id)
      seeds = info.seeds.map((s) => ({ ...s }))
    } catch (e) { error = String(e) }
  }

  $effect(() => {
    // Reload when another playlist is chosen.
    brief = playlist.brief
    editingSeeds = false
    message = ''
    void load()
  })

  async function run(fn: () => Promise<unknown>, done: string) {
    error = ''; message = ''
    try { await fn(); message = done; onchange?.() } catch (e) { error = String(e) }
    await load()
  }
</script>

<div class="workspace">
  <label class="brief">
    <span>Brief</span>
    <textarea rows="2" maxlength="2000" bind:value={brief}
      placeholder="What this playlist is for, e.g. Warm-up set: dubby, spacious, restrained vocals"></textarea>
  </label>
  {#if brief !== playlist.brief}
    <div class="row">
      <button onclick={() => run(() => workspace.setBrief(playlist.id, brief), 'Brief saved.')}>Save brief</button>
      <button onclick={() => (brief = playlist.brief)}>Undo changes</button>
    </div>
  {/if}

  <div class="seeds">
    <span>Seeds</span>
    {#if editingSeeds}
      {#each seeds as seed, i}
        <div class="seed">
          <select aria-label="Seed kind" bind:value={seed.kind}>
            {#each ['artist', 'label', 'dj', 'track'] as kind}<option value={kind}>{kind}</option>{/each}
          </select>
          <input aria-label="Seed value" bind:value={seed.value} />
          <button aria-label="Remove seed" onclick={() => (seeds = seeds.filter((_, j) => j !== i))}>Remove</button>
        </div>
      {/each}
      <div class="row">
        <button onclick={() => (seeds = [...seeds, { kind: 'artist', value: '' }])}>Add seed</button>
        <button onclick={() => run(async () => { await workspace.saveSeeds(playlist.id, seeds.filter((s) => s.value.trim())); editingSeeds = false }, 'Seeds saved.')}>Save seeds</button>
        <button onclick={() => { editingSeeds = false; seeds = (info?.seeds ?? []).map((s) => ({ ...s })) }}>Cancel</button>
      </div>
    {:else}
      <span class="muted">
        {info?.seeds.length ? info.seeds.map((s) => `${s.value} (${s.kind})`).join(', ') : 'None yet. The artists and labels of tracks in the playlist are used too.'}
      </span>
      <button onclick={() => (editingSeeds = true)}>Edit seeds</button>
    {/if}
  </div>

  <div class="row discovery">
    <label class="check">
      <input type="checkbox" checked={playlist.discovery}
        onchange={(e) => { const on = e.currentTarget.checked; void run(() => workspace.setDiscovery(playlist.id, on), on ? 'The app will look for tracks for this playlist.' : 'Paused.') }} />
      Look for tracks for this playlist in the background
    </label>
    <button onclick={() => run(() => workspace.discoverNow(playlist.id), 'Looking for tracks. Suggestions appear once their audio is ready.')}>Find tracks now</button>
    {#if info}
      <button class="primary" disabled={!info.ready} onclick={() => reviewFor(playlist.id)}>
        {info.ready ? `Review ${info.ready} suggestions` : 'No suggestions waiting'}
      </button>
    {/if}
  </div>
  <p class="muted small">Suggestions are added only when you choose Add to playlist, always at the end. Nothing changes your order.</p>

  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}

  {#if info?.runs.length}
    <details>
      <summary>Recent searches for this playlist</summary>
      <ul class="runs">
        {#each info.runs as r}
          <li>
            {formatRelative(r.finished_at)}: {r.outcome === 'failed' ? `failed (${r.detail ?? 'no detail'})` : r.outcome === 'empty' ? 'nothing new' : `${r.created} new, ${r.already_known} already known`}
          </li>
        {/each}
      </ul>
    </details>
  {/if}
</div>

<style>
  .workspace { display: grid; gap: 8px; margin: 12px 0 16px; padding: 12px; border: 1px solid var(--border); border-radius: 6px; }
  .brief { display: grid; gap: 4px; }
  textarea { width: 100%; resize: vertical; font: inherit; }
  .seeds { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .seed, .row { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
  .seed input { flex: 1; min-width: 160px; }
  .check { display: flex; gap: 6px; align-items: center; }
  .runs { margin: 4px 0; padding-left: 18px; font-size: 13px; }
  .small { font-size: 12px; margin: 0; }
</style>
