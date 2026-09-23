<script lang="ts">
  // Waveform overview that doubles as a seek bar. Falls back to a plain bar
  // when no waveform is available.
  let {
    peaks,
    positionMs,
    durationMs,
    onseek,
    height = 48,
  }: {
    peaks: number[] | null
    positionMs: number
    durationMs: number | null
    onseek: (ms: number) => void
    height?: number
  } = $props()

  let canvas: HTMLCanvasElement | undefined = $state()
  let width = $state(600)

  const progress = $derived(durationMs ? Math.min(1, positionMs / durationMs) : 0)

  $effect(() => {
    if (!canvas) return
    const dpr = window.devicePixelRatio || 1
    canvas.width = width * dpr
    canvas.height = height * dpr
    const ctx = canvas.getContext('2d')!
    ctx.scale(dpr, dpr)
    ctx.clearRect(0, 0, width, height)
    const styles = getComputedStyle(canvas)
    const played = styles.getPropertyValue('--accent').trim() || '#f0a53a'
    const rest = styles.getPropertyValue('--border').trim() || '#444'
    const split = progress * width
    if (!peaks || peaks.length === 0) {
      ctx.fillStyle = rest
      ctx.fillRect(0, height / 2 - 2, width, 4)
      ctx.fillStyle = played
      ctx.fillRect(0, height / 2 - 2, split, 4)
      return
    }
    const bar = width / peaks.length
    for (let i = 0; i < peaks.length; i++) {
      const x = i * bar
      const h = Math.max(1, (peaks[i] / 255) * height)
      ctx.fillStyle = x < split ? played : rest
      ctx.fillRect(x, (height - h) / 2, Math.max(1, bar - 0.5), h)
    }
  })

  function click(e: MouseEvent) {
    if (!durationMs || !canvas) return
    const rect = canvas.getBoundingClientRect()
    onseek(((e.clientX - rect.left) / rect.width) * durationMs)
  }
</script>

<div class="wave" bind:clientWidth={width} style:height="{height}px">
  <canvas
    bind:this={canvas}
    style:width="{width}px"
    style:height="{height}px"
    onclick={click}
    aria-label="Seek"
  ></canvas>
</div>

<style>
  .wave {
    width: 100%;
    cursor: pointer;
  }
</style>
