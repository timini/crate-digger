<script lang="ts">
  import { onMount } from 'svelte'
  import { metadata, type MetadataSuggestion } from '../lib/api'
  import { trackLabel } from '../lib/format'

  let { onchange }: { onchange: () => void } = $props()
  let suggestions: MetadataSuggestion[] = $state([])
  let message = $state('')
  let error = $state('')

  async function load() {
    try {
      suggestions = (await metadata.suggestions()) ?? []
    } catch (e) {
      error = String(e)
    }
  }
  onMount(load)

  async function act(action: () => Promise<unknown>, done?: string) {
    error = ''
    try {
      await action()
      if (done) message = done
      await load()
      onchange()
    } catch (e) {
      error = String(e)
    }
  }

  // One entry per track, with its possible matches best first.
  const byTrack = $derived(
    Object.values(
      suggestions.reduce<Record<string, MetadataSuggestion[]>>((acc, s) => {
        ;(acc[s.track_id] ??= []).push(s)
        return acc
      }, {}),
    ),
  )
  const field = (s: MetadataSuggestion, f: string) => s.found.fields.find(([k]) => k === f)?.[1] ?? ''
</script>

<div class="metadata-check">
  <button
    onclick={() =>
      act(async () => {
        const n = await metadata.identifyAll()
        message = n ? `Looking up ${n} tracks in the background. Progress is in Activity.` : 'Every analysed track has been looked up.'
      })}>Identify all</button
  >
  {#if message}<span class="muted small" role="status">{message}</span>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if byTrack.length}
    <details>
      <summary>{byTrack.length} {byTrack.length === 1 ? 'track needs' : 'tracks need'} a metadata check</summary>
      {#each byTrack as options (options[0].track_id)}
        <section>
          <p>Now: <strong>{trackLabel(options[0].current)}</strong></p>
          {#each options as s (s.id)}
            <div class="option">
              {field(s, 'artist')} - {field(s, 'title')}{field(s, 'mix') ? ` (${field(s, 'mix')})` : ''}
              {#if field(s, 'release')}<span class="muted small"> · {field(s, 'release')}</span>{/if}
              <span class="muted small"> · match {Math.round(s.score * 100)}%</span>
              <button onclick={() => act(() => metadata.accept(s.id))}>Use this</button>
            </div>
          {/each}
          <button onclick={() => act(() => metadata.dismiss(options[0].track_id))}>Keep as it is</button>
        </section>
      {/each}
    </details>
  {/if}
</div>

<style>
  .metadata-check { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  details { flex-basis: 100%; }
  section { margin: 8px 0; }
  .option { display: flex; gap: 6px; align-items: baseline; margin: 2px 0; }
</style>
