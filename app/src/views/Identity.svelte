<script lang="ts">
  import { onMount } from 'svelte'
  import { api, type Conflict, type ConflictSide, type IdentityEvidence, type Relation } from '../lib/api'
  import { formatDuration, trackLabel } from '../lib/format'
  import { playTrack } from '../lib/player.svelte'

  let { onchange }: { onchange?: () => void } = $props()

  let conflicts: Conflict[] = $state([])
  let error: string | null = $state(null)
  let loaded = $state(false)

  async function load() {
    try {
      conflicts = await api.conflicts()
      error = null
    } catch (e) {
      error = String(e)
    }
    loaded = true
  }

  async function resolve(c: Conflict, relation: Relation) {
    try {
      await api.resolveConflict(c.id, relation)
    } catch (e) {
      error = String(e)
    }
    await load()
    onchange?.()
  }

  function describe(e: IdentityEvidence): string {
    switch (e.kind) {
      case 'fingerprint_match':
        return `Audio matches: ${Math.round(e.score * 100)}% alignment over ${Math.round(e.coverage * 100)}% of the track` +
          (Math.abs(e.speed - 1) > 0.005 ? `, when played ${((e.speed - 1) * 100).toFixed(1)}% faster` : '')
      case 'fingerprint_mismatch':
        return 'Audio does not match'
      case 'identical_bytes':
        return 'Identical files'
      case 'embedding_similarity':
        return `Sounds similar (${Math.round(e.similarity * 100)}%), which is not proof of identity`
      case 'llm_assertion':
        return `AI suggestion: ${e.claim} (not proof)`
      case 'user_decision':
        return 'You decided'
    }
  }

  onMount(load)
</script>

{#snippet side(label: string, s: ConflictSide)}
  <div class="side">
    <div class="muted small">{label}</div>
    <strong>{trackLabel(s.meta)}</strong>
    <div class="muted small">{formatDuration(s.duration_ms)} · <span class="path" title={s.path ?? ''}>{s.path ?? 'no audio'}</span></div>
    {#if s.path}
      <button onclick={() => playTrack(s.track_id, trackLabel(s.meta))}>Play {label}</button>
    {/if}
  </div>
{/snippet}

<section>
  <h2>Identity review</h2>
  <p class="muted">
    These pairs have conflicting evidence: for example the audio matches but the tags name different mixes. Listen and
    decide. Merging combines files, ratings and playlist entries; nothing on disk changes.
  </p>
  {#if error}<p class="error">{error}</p>{/if}
  {#if loaded && conflicts.length === 0}
    <p class="muted">Nothing to review.</p>
  {/if}
  {#each conflicts as c (c.id)}
    <article class="conflict">
      <p class="reason">{c.reason}</p>
      <div class="sides">
        {@render side('A', c.a)}
        {@render side('B', c.b)}
      </div>
      {#if c.evidence.length}
        <ul class="evidence">
          {#each c.evidence as e}<li>{describe(e)}</li>{/each}
        </ul>
      {/if}
      <div class="actions">
        <button onclick={() => resolve(c, 'same_recording')}>Same recording (merge into A)</button>
        <button onclick={() => resolve(c, 'different_version')}>Different versions</button>
        <button onclick={() => resolve(c, 'unrelated')}>Not related</button>
      </div>
    </article>
  {/each}
</section>

<style>
  section {
    max-width: 900px;
  }
  .conflict {
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px 16px;
    margin: 12px 0;
    background: var(--panel);
  }
  .reason {
    margin-top: 0;
    font-weight: 600;
  }
  .sides {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
  }
  .side {
    display: grid;
    gap: 4px;
    align-content: start;
  }
  .side button {
    justify-self: start;
  }
  .path {
    word-break: break-all;
  }
  .small {
    font-size: 12px;
  }
  .evidence {
    margin: 10px 0;
    padding-left: 18px;
    color: var(--muted);
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
</style>
