<script lang="ts">
  import { onMount } from 'svelte'
  import { connections, type DownloadChoice, type DownloadOption } from '../lib/api'
  import { formatDuration, trackLabel } from '../lib/format'

  let choices: DownloadChoice[] = $state([])
  let error = $state('')
  let busy = $state(false)

  async function load() {
    try {
      choices = (await connections.downloadChoices()) ?? []
    } catch (e) {
      error = String(e)
    }
  }
  onMount(load)

  async function act(action: () => Promise<void>) {
    busy = true
    error = ''
    try {
      await action()
    } catch (e) {
      error = String(e)
    } finally {
      busy = false
      await load()
    }
  }

  function file(o: DownloadOption) {
    return o.result.filename.split(/[\\/]/).pop() ?? o.result.filename
  }
  function quality(o: DownloadOption) {
    if (o.quality === 'lossless') return (o.result.format ?? 'lossless').toUpperCase()
    const kbps = o.result.bitrate_kbps ? ` ${o.result.bitrate_kbps} kbps` : ''
    return `${(o.result.format ?? '').toUpperCase()}${kbps}`
  }
</script>

{#if choices.length}
  <details class="choices" open>
    <summary>{choices.length} {choices.length === 1 ? 'download needs' : 'downloads need'} your choice</summary>
    {#each choices as c (c.candidate_id)}
      <section>
        <h4>{trackLabel(c.meta)}</h4>
        <p class="muted small">{c.why}</p>
        <table>
          <tbody>
            {#each c.outcome.ranked as o, i (o.result.result_id)}
              <tr class:recommended={c.outcome.decision.recommended === i}>
                <td>
                  {file(o)}
                  {#if c.outcome.decision.recommended === i}<span class="badge">Recommended</span>{/if}
                  <div class="muted small">
                    {o.result.username ?? ''}{o.result.free_slot ? ' · free slot' : o.result.queue_length ? ` · ${o.result.queue_length} queued` : ''}
                  </div>
                </td>
                <td>{quality(o)}</td>
                <td>{o.result.duration_ms ? formatDuration(o.result.duration_ms) : 'length unknown'}</td>
                <td class="muted small">{o.notes.join(', ')}</td>
                <td><button disabled={busy} onclick={() => act(() => connections.downloadChoose(c.candidate_id, o.result.result_id))}>Download</button></td>
              </tr>
            {/each}
          </tbody>
        </table>
        <button disabled={busy} onclick={() => act(() => connections.downloadDecline(c.candidate_id))}>None of these</button>
      </section>
    {/each}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
  </details>
{/if}

<style>
  .choices { margin: 8px 0 16px; text-align: left; }
  section { margin: 12px 0; }
  table { width: 100%; border-collapse: collapse; }
  td { padding: 4px 8px 4px 0; vertical-align: top; }
  tr.recommended td { font-weight: 600; }
  .badge { margin-left: 6px; font-size: 11px; border: 1px solid var(--border); border-radius: 4px; padding: 0 4px; }
</style>
