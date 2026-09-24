<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog'
  import { revealItemInDir } from '@tauri-apps/plugin-opener'
  import { api, type Field, type RelinkProposal, type TrackDetail } from '../lib/api'
  import { formatBytes, formatCodec, formatDuration, formatVariant, trackLabel } from '../lib/format'
  import RatingControl from './RatingControl.svelte'
  import { playTrack } from '../lib/player.svelte'
  import AddToPlaylist from './AddToPlaylist.svelte'

  let { trackId, onclose, onchange }: { trackId: string; onclose: () => void; onchange: () => void } = $props()

  const fields: { field: Field; label: string }[] = [
    { field: 'artist', label: 'Artist' },
    { field: 'title', label: 'Title' },
    { field: 'mix', label: 'Mix' },
    { field: 'label', label: 'Label' },
    { field: 'release', label: 'Release' },
    { field: 'track_number', label: 'Track no.' },
    { field: 'year', label: 'Year' },
    { field: 'genre', label: 'Genre' },
    { field: 'tempo', label: 'BPM' },
    { field: 'musical_key', label: 'Key' },
  ]

  let detail: TrackDetail | null = $state(null)
  let error: string | null = $state(null)
  let proposals: RelinkProposal[] = $state([])

  async function load(id: string) {
    try {
      detail = await api.trackDetail(id)
      error = null
    } catch (e) {
      error = String(e)
    }
  }

  $effect(() => {
    proposals = []
    load(trackId)
  })

  function value(f: Field): string {
    const v = detail?.meta[f]
    return v == null ? '' : String(v)
  }

  function corrected(f: Field): boolean {
    return detail?.provenance.find((p) => p.field === f)?.corrected ?? false
  }

  async function save(f: Field, input: HTMLInputElement) {
    if (input.value === value(f)) return
    try {
      await api.setField(trackId, f, input.value)
      await load(trackId)
      onchange()
    } catch (e) {
      error = String(e)
    }
  }

  async function revert(f: Field) {
    await api.setField(trackId, f, null)
    await load(trackId)
    onchange()
  }

  async function relinkPick(fileId: string) {
    const path = await open({ multiple: false, title: 'Choose the moved file' })
    if (typeof path !== 'string') return
    try {
      await api.relinkApply(fileId, path)
      await load(trackId)
      onchange()
    } catch (e) {
      error = String(e)
    }
  }

  async function relinkSearch() {
    const folder = await open({ directory: true, multiple: false, title: 'Where might the missing files be?' })
    if (typeof folder !== 'string') return
    try {
      proposals = await api.relinkFind(folder)
      if (proposals.length === 0) error = 'No matching files found in that folder.'
    } catch (e) {
      error = String(e)
    }
  }

  async function applyProposal(p: RelinkProposal) {
    await api.relinkApply(p.file_id, p.new_path)
    proposals = proposals.filter((x) => x !== p)
    await load(trackId)
    onchange()
  }
</script>

<aside>
  <header>
    <h3>Track</h3>
    <button onclick={onclose} aria-label="Close">Close</button>
  </header>
  {#if error}<p class="error">{error}</p>{/if}
  {#if detail}
    <p class="muted">
      <RatingControl trackId={trackId} rating={detail.rating} onchange={(r) => ((detail!.rating = r), onchange())} />
      {#if detail.kept} · Kept{/if}
      {#if detail.playlists.length}· In {detail.playlists.map((p) => p[1]).join(', ')}{/if}
    </p>

    {#key trackId}
      <AddToPlaylist trackIds={[trackId]} onadded={() => (load(trackId), onchange())} />
    {/key}

    <form onsubmit={(e) => e.preventDefault()}>
      {#each fields as f (f.field)}
        <label>
          <span>{f.label}</span>
          <input
            value={value(f.field)}
            onblur={(e) => save(f.field, e.currentTarget)}
            onkeydown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
          />
          {#if corrected(f.field)}
            <button type="button" class="link" title="Use the value from the file again" onclick={() => revert(f.field)}>Edited · revert</button>
          {/if}
        </label>
      {/each}
    </form>
    <p class="muted small">Edits are stored separately and are never overwritten by a rescan. Files are not modified.</p>

    <h4>Analysis</h4>
    {#if detail.analysis.state?.state === 'done'}
      <p class="small">
        {#if detail.analysis.tempo}{detail.analysis.tempo} BPM{/if}
        {#if detail.analysis.key} · {detail.analysis.key.name} ({detail.analysis.key.camelot}){/if}
        {#if detail.analysis.loudness_lufs != null} · {detail.analysis.loudness_lufs.toFixed(1)} LUFS{/if}
      </p>
      <p class="muted small">Estimated by {detail.analysis.model}. Tags and your edits take precedence in the fields above.</p>
    {:else if detail.analysis.state}
      <p class="small">{detail.analysis.state.state === 'queued' ? 'Waiting to be analysed.' : detail.analysis.state.reason}</p>
    {:else}
      <p class="muted small">Not analysed yet.</p>
    {/if}

    {#if detail.versions.length}
      <h4>Other versions</h4>
      {#each detail.versions as v (v.track_id)}
        <div class="version">
          <span>{trackLabel(v.meta)}</span>
          {#if v.has_audio}<button onclick={() => playTrack(v.track_id, trackLabel(v.meta))}>Play</button>{/if}
        </div>
      {/each}
    {/if}

    <h4>Files</h4>
    {#each detail.files as file (file.id)}
      <div class="file" class:bad={file.availability !== 'available'}>
        <div class="path" title={file.path}>{file.path}</div>
        <div class="muted small">
          {formatCodec(file.format)} · {formatDuration(file.duration_ms)} · {formatBytes(file.size_bytes)}
          {#if file.bitrate_kbps} · {file.bitrate_kbps} kbps{/if}
          {#if file.is_primary} · primary{/if}
          {#if file.variant} · {formatVariant(file.variant)}{/if}
        </div>
        {#if file.availability_reason}<div class="reason">{file.availability_reason}</div>{/if}
        <div class="actions">
          {#if file.availability !== 'missing'}
            <button onclick={() => revealItemInDir(file.path)}>Show in folder</button>
          {/if}
          {#if !file.is_primary}
            <button onclick={() => api.setPrimaryFile(file.id).then(() => load(trackId))}>Make primary</button>
          {/if}
          {#if file.availability !== 'available'}
            <button onclick={() => relinkPick(file.id)}>Relink...</button>
          {/if}
        </div>
      </div>
    {/each}
    {#if detail.files.some((f) => f.availability === 'missing')}
      <button onclick={relinkSearch}>Search a folder for missing files...</button>
    {/if}
    {#each proposals as p}
      <div class="file">
        <div class="small">Found {p.method === 'content' ? 'an identical file' : 'a file with the same name and length'}:</div>
        <div class="path" title={p.new_path}>{p.new_path}</div>
        <button class="primary" onclick={() => applyProposal(p)}>Relink</button>
      </div>
    {/each}
  {/if}
</aside>

<style>
  aside {
    background: var(--panel);
    border-left: 1px solid var(--border);
    padding: 12px 16px;
    overflow: auto;
    margin: -20px -24px -20px 0;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  form {
    margin-top: 12px;
    display: grid;
    gap: 6px;
  }
  label {
    display: grid;
    grid-template-columns: 80px 1fr;
    align-items: center;
    gap: 8px;
  }
  label .link {
    grid-column: 2;
    justify-self: start;
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font-size: 12px;
  }
  .small {
    font-size: 12px;
  }
  .version {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
  }
  .file {
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    margin-bottom: 8px;
  }
  .file.bad {
    border-color: var(--danger);
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .reason {
    color: var(--danger);
    font-size: 12px;
    margin: 4px 0;
  }
  .actions {
    display: flex;
    gap: 6px;
    margin-top: 6px;
  }
</style>
