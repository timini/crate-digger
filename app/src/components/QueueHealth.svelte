<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { connections, type QueueHealth } from '../lib/api'

  let health: QueueHealth | null = $state(null)
  let timer: ReturnType<typeof setInterval> | undefined

  async function load() {
    try {
      health = (await connections.queueHealth()) ?? null
    } catch {
      health = null
    }
  }
  onMount(() => {
    void load()
    timer = setInterval(load, 15000)
  })
  onDestroy(() => clearInterval(timer))

  const pct = (x: number) => `${Math.round(x * 100)}%`
</script>

{#if health}
  <div class="health">
    {#if health.buffer.ready < health.buffer.below}
      <p class="muted small">
        {health.buffer.ready} of {health.buffer.target} ready, {health.buffer.in_progress} on the way.
      </p>
    {/if}
    {#each health.holds as h (h.code)}
      <p class="hold" data-code={h.code}>{h.message}</p>
    {/each}
    {#if health.availability.samples > 0}
      <p class="muted small">
        Last 7 days: tracks ready {pct(health.availability.any_ready)} of the time, at least {health.buffer.below} ready
        {pct(health.availability.above_threshold)}.
      </p>
    {/if}
  </div>
{/if}

<style>
  .health { margin: 4px 0 8px; }
  .hold { margin: 2px 0; font-size: 13px; }
</style>
