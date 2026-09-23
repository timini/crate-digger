<script lang="ts">
  import { save } from '@tauri-apps/plugin-dialog'
  import { exporting, type ExportCheck } from '../lib/api'

  let { id }: { id: string } = $props()
  let check = $state<ExportCheck | null>(null)
  let message = $state('')
  let error = $state('')

  async function start() {
    error = ''
    message = ''
    try {
      check = await exporting.check(id)
    } catch (e) {
      error = String(e)
    }
  }

  async function write(format: 'm3u8' | 'rekordbox') {
    if (!check) return
    const ext = format === 'm3u8' ? 'm3u8' : 'xml'
    const path = await save({
      defaultPath: `${check.name}.${ext}`,
      filters: [{ name: format === 'm3u8' ? 'M3U8 playlist' : 'Rekordbox XML', extensions: [ext] }],
    })
    if (!path) return
    try {
      const n = await exporting.write(id, format, path)
      message = `Exported ${n} tracks to ${path}.`
      check = null
    } catch (e) {
      error = String(e)
    }
  }

  const missing = $derived(check?.problems.filter((p) => p.kind === 'missing') ?? [])
  const temporary = $derived(check?.problems.filter((p) => p.kind === 'temporary') ?? [])
</script>

{#if check}
  <div class="export">
    {#if missing.length}
      <p class="error">{missing.length} {missing.length === 1 ? 'track has' : 'tracks have'} no available audio and will be left out:</p>
      <ul>{#each missing as p}<li>{p.position + 1}. {p.track}</li>{/each}</ul>
    {/if}
    {#if temporary.length}
      <p>
        {temporary.length} {temporary.length === 1 ? 'track is' : 'tracks are'} still in temporary storage. Keep them
        first if you can: keeping moves the file to your archive, which would break the exported path.
      </p>
      <ul>{#each temporary as p}<li>{p.position + 1}. {p.track}</li>{/each}</ul>
    {/if}
    <p class="muted small">
      {check.tracks.length} tracks will be exported in playlist order. No cue points or beat grids are written; tempo
      and key only when they come from file tags or your edits.
    </p>
    <button disabled={!check.tracks.length} onclick={() => write('rekordbox')}>Save Rekordbox XML</button>
    <button disabled={!check.tracks.length} onclick={() => write('m3u8')}>Save M3U8</button>
    <button onclick={() => (check = null)}>Cancel</button>
  </div>
{:else}
  <button onclick={start}>Export</button>
{/if}
{#if message}<p role="status">{message}</p>{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}

<style>
  .export { border: 1px solid var(--border); border-radius: 8px; padding: 8px 12px; margin: 8px 0; }
  ul { margin: 4px 0 8px; }
</style>
