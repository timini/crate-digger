<script lang="ts">
  import { api, type RatingKind } from '../lib/api'

  let {
    trackId,
    rating,
    onchange,
  }: { trackId: string; rating: RatingKind | null; onchange: (r: RatingKind | null) => void } = $props()

  const options: { kind: RatingKind; label: string; title: string }[] = [
    { kind: 'thumbs_down', label: 'Down', title: 'Thumbs down (0)' },
    { kind: 'star1', label: '★', title: 'One star (1)' },
    { kind: 'star2', label: '★★', title: 'Two stars (2)' },
    { kind: 'star3', label: '★★★', title: 'Three stars (3)' },
  ]
  let error = $state('')

  async function set(kind: RatingKind | null) {
    error = ''
    try {
      // Choosing the current rating again clears it.
      const next = kind === rating ? null : kind
      await api.libraryRate(trackId, next)
      onchange(next)
    } catch (e) {
      error = String(e)
    }
  }
</script>

<span class="rating-control" role="group" aria-label="Rating">
  {#each options as o (o.kind)}
    <button
      class:on={rating === o.kind}
      title={o.title}
      aria-label={o.title}
      aria-pressed={rating === o.kind}
      onclick={(e) => (e.stopPropagation(), set(o.kind))}>{o.label}</button
    >
  {/each}
  {#if error}<span class="error" role="alert">{error}</span>{/if}
</span>

<style>
  .rating-control { display: inline-flex; gap: 2px; white-space: nowrap; }
  button { padding: 0 4px; border: 1px solid transparent; background: none; opacity: 0.35; font-size: 12px; }
  button:hover { opacity: 0.8; }
  button.on { opacity: 1; border-color: var(--border); }
</style>
