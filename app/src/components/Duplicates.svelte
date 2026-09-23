<script lang="ts">
  import { onMount } from 'svelte'
  import { api, type DuplicatePair } from '../lib/api'
  import { formatDuration } from '../lib/format'

  let { onclose }: { onclose: () => void } = $props()
  let pairs: DuplicatePair[] = $state([])
  let loading = $state(true)
  let error: string | null = $state(null)

  async function load() {
    try {
      pairs = await api.duplicates()
    } catch (e) {
      error = String(e)
    }
    loading = false
  }

  async function act(fn: () => Promise<void>) {
    try {
      await fn()
    } catch (e) {
      error = String(e)
    }
    await load()
  }

  onMount(load)
</script>

<div class="backdrop" role="presentation" onclick={onclose}>
  <div class="dialog" role="dialog" aria-label="Possible duplicates" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.key === 'Escape' && onclose()}>
    <header>
      <h3>Possible duplicates</h3>
      <button onclick={onclose}>Close</button>
    </header>
    <p class="muted">
      Same artist, title and mix with a similar length. Merging combines their files, ratings and playlist entries into
      one track. Nothing on disk changes.
    </p>
    {#if error}<p class="error">{error}</p>{/if}
    {#if loading}
      <p class="muted">Looking...</p>
    {:else if pairs.length === 0}
      <p class="muted">No possible duplicates.</p>
    {/if}
    {#each pairs as p (p.track_a + p.track_b)}
      <div class="pair">
        <strong>{p.artist} - {p.title}{p.mix ? ` (${p.mix})` : ''}</strong>
        <div class="files">
          <div><span class="muted">A</span> {p.path_a} <span class="muted">{formatDuration(p.duration_a_ms)}</span></div>
          <div><span class="muted">B</span> {p.path_b} <span class="muted">{formatDuration(p.duration_b_ms)}</span></div>
        </div>
        <div class="actions">
          <button onclick={() => act(() => api.mergeDuplicates(p.track_a, p.track_b))}>Same track, keep A's details</button>
          <button onclick={() => act(() => api.mergeDuplicates(p.track_b, p.track_a))}>Same track, keep B's details</button>
          <button onclick={() => act(() => api.dismissDuplicates(p.track_a, p.track_b))}>Different tracks</button>
        </div>
      </div>
    {/each}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.5);
    display: grid;
    place-items: center;
    z-index: 10;
  }
  .dialog {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 16px 20px;
    width: min(760px, 90vw);
    max-height: 80vh;
    overflow: auto;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .pair {
    border-top: 1px solid var(--border);
    padding: 10px 0;
  }
  .files {
    font-size: 12px;
    margin: 6px 0;
    word-break: break-all;
  }
  .actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
</style>
