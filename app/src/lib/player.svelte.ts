// Shared player state. The Rust player is the source of truth; this polls
// it and smooths the position between polls.
import { api, type NowPlaying } from './api'

export const player = $state({
  trackId: null as string | null,
  state: 'idle' as NonNullable<NowPlaying['state']>,
  positionMs: 0,
  durationMs: null as number | null,
  volume: 1,
  error: null as string | null,
  label: '' as string,
  waveform: null as number[] | null,
})

let lastPoll = 0
let lastPosition = 0
let loadedWaveformFor: string | null = null

async function poll() {
  try {
    const s = await api.playerStatus()
    player.trackId = s.track_id
    player.state = s.state ?? 'idle'
    player.durationMs = s.duration_ms ?? null
    player.volume = s.volume ?? player.volume
    if (s.error) player.error = s.error
    lastPosition = s.position_ms ?? 0
    lastPoll = performance.now()
    player.positionMs = lastPosition
    if (s.track_id && s.track_id !== loadedWaveformFor) {
      loadedWaveformFor = s.track_id
      player.waveform = null
      api.waveform(s.track_id).then((w) => {
        if (loadedWaveformFor === s.track_id) player.waveform = w
      })
    }
  } catch {
    // The status command only fails if the app is shutting down.
  }
}

function tick() {
  if (player.state === 'playing') {
    const p = lastPosition + (performance.now() - lastPoll)
    player.positionMs = player.durationMs ? Math.min(p, player.durationMs) : p
  }
  requestAnimationFrame(tick)
}

let started = false
export function startPlayerPolling() {
  if (started) return
  started = true
  setInterval(poll, 250)
  requestAnimationFrame(tick)
  poll()
}

export async function playTrack(trackId: string, label: string, startMs?: number) {
  player.error = null
  player.label = label
  try {
    await api.playTrack(trackId, startMs)
  } catch (e) {
    player.error = String(e)
  }
  await poll()
}

export async function togglePlay() {
  if (!player.trackId) return
  await api.togglePlay()
  await poll()
}

export async function seekTo(ms: number) {
  if (!player.trackId) return
  lastPosition = ms
  lastPoll = performance.now()
  player.positionMs = ms
  await api.seek(ms)
}

export async function seekBy(deltaMs: number) {
  await seekTo(Math.max(0, player.positionMs + deltaMs))
}

export async function setVolume(v: number) {
  player.volume = v
  await api.setVolume(v)
}
