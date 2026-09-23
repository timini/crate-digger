<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { api, type ModelStatus } from '../lib/api'
  import { formatBytes } from '../lib/format'

  let models: ModelStatus[] = $state([])
  let downloading: string | null = $state(null)
  let bytes = $state(0)
  let error: string | null = $state(null)
  let timer: ReturnType<typeof setInterval> | undefined

  async function load() {
    models = await api.models()
  }

  async function download(m: ModelStatus) {
    downloading = m.id
    bytes = 0
    error = null
    timer = setInterval(async () => {
      const p = await api.downloadProgress()
      if (p) bytes = p.bytes
    }, 300)
    try {
      await api.downloadModel(m.id)
    } catch (e) {
      error = String(e)
    }
    clearInterval(timer)
    downloading = null
    await load()
  }

  async function choose(id: string | null) {
    try {
      await api.chooseModel(id)
      error = null
    } catch (e) {
      error = String(e)
    }
    await load()
  }

  onMount(load)
  onDestroy(() => clearInterval(timer))

  const builtIn = $derived(!models.some((m) => m.chosen))
</script>

<div class="models">
  <label class="row">
    <input type="radio" name="model" checked={builtIn} onchange={() => choose(null)} />
    <span><strong>Built-in (cd-dsp-v1)</strong> <span class="muted">No download. A spectral and harmonic summary; always computed.</span></span>
  </label>
  {#each models as m (m.id)}
    <div class="row">
      <input
        type="radio"
        name="model"
        checked={m.chosen}
        disabled={!m.installed}
        onchange={() => choose(m.id)}
        aria-label={`Use ${m.id}`}
      />
      <span class="info">
        <strong>{m.id}{#if m.recommended} <span class="rec">Recommended</span>{/if}</strong>
        <span class="muted">{m.dims} dimensions · {formatBytes(m.size_bytes)} · {m.licence}</span>
      </span>
      {#if downloading === m.id}
        <span class="muted">{formatBytes(bytes)} of {formatBytes(m.size_bytes)}</span>
      {:else if !m.installed}
        <button onclick={() => download(m)} disabled={downloading !== null}>Download</button>
      {:else}
        <span class="muted">Installed</span>
      {/if}
    </div>
  {/each}
  {#if error}<p class="error">{error}</p>{/if}
  <p class="muted small">
    Models are downloaded only when you ask, checked against a pinned checksum, and run in a separate process. Choosing a
    model re-analyses tracks whose audio is available; earlier results are kept. Non-commercial licences allow use in this
    free app but not in commercial products.
  </p>
</div>

<style>
  .row {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 4px 0;
  }
  .info {
    flex: 1;
    display: grid;
  }
  .rec {
    font-size: 11px;
    color: var(--accent);
    font-weight: 500;
  }
  .small {
    font-size: 12px;
  }
</style>
