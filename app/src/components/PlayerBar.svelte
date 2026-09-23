<script lang="ts">
  import { onMount } from 'svelte'
  import { player, seekBy, seekTo, setVolume, startPlayerPolling, togglePlay } from '../lib/player.svelte'
  import { formatDuration } from '../lib/format'
  import Waveform from './Waveform.svelte'

  onMount(startPlayerPolling)

  // Space toggles playback anywhere except in text fields. Views that
  // handle Space themselves call preventDefault first.
  function onKey(e: KeyboardEvent) {
    if (e.defaultPrevented) return
    const t = e.target as HTMLElement
    if (['INPUT', 'SELECT', 'TEXTAREA', 'BUTTON'].includes(t.tagName)) return
    if (e.key === ' ') {
      e.preventDefault()
      togglePlay()
    } else if (e.key === 'ArrowRight' && e.shiftKey) {
      seekBy(10_000)
    } else if (e.key === 'ArrowLeft' && e.shiftKey) {
      seekBy(-10_000)
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<footer class="player">
  <button class="play" onclick={togglePlay} disabled={!player.trackId} aria-label={player.state === 'playing' ? 'Pause' : 'Play'}>
    {player.state === 'playing' ? 'Pause' : 'Play'}
  </button>
  <div class="info">
    <div class="label">
      {#if player.error}
        <span class="error">{player.error}</span>
      {:else}
        {player.label || (player.trackId ? 'Now playing' : 'Nothing playing')}
      {/if}
    </div>
    <Waveform peaks={player.waveform} positionMs={player.positionMs} durationMs={player.durationMs} onseek={seekTo} height={36} />
  </div>
  <div class="time">{formatDuration(player.positionMs)} / {formatDuration(player.durationMs)}</div>
  <label class="volume">
    <span class="muted">Vol</span>
    <input
      type="range"
      min="0"
      max="1"
      step="0.01"
      value={player.volume}
      oninput={(e) => setVolume(Number(e.currentTarget.value))}
      aria-label="Volume"
    />
  </label>
</footer>

<style>
  .player {
    display: grid;
    grid-template-columns: auto 1fr auto auto;
    gap: 16px;
    align-items: center;
    padding: 8px 16px;
    background: var(--panel);
    border-top: 1px solid var(--border);
  }
  .play {
    width: 72px;
  }
  .info {
    min-width: 0;
  }
  .label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 13px;
    margin-bottom: 2px;
  }
  .time {
    font-variant-numeric: tabular-nums;
    color: var(--muted);
  }
  .volume {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .volume input {
    width: 100px;
  }
</style>
