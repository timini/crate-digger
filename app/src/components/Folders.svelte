<script lang="ts">
  // Music folder list shared by Settings and onboarding.
  import { onMount } from 'svelte'
  import { open } from '@tauri-apps/plugin-dialog'
  import { api, type LibraryRoot } from '../lib/api'

  let roots: LibraryRoot[] = $state([])
  let error: string | null = $state(null)

  async function load() {
    roots = await api.roots()
  }

  async function add() {
    const dir = await open({ directory: true, multiple: false, title: 'Choose a music folder' })
    if (typeof dir !== 'string') return
    try {
      await api.addRoot(dir)
      error = null
    } catch (e) {
      error = String(e)
    }
    await load()
  }

  async function remove(id: string) {
    await api.removeRoot(id)
    await load()
  }

  onMount(load)
</script>

<div class="folders">
  {#each roots as r (r.id)}
    <div class="row">
      <span class="path">{r.path}</span>
      <span class="muted">{r.file_count} files</span>
      <button onclick={() => remove(r.id)}>Remove</button>
    </div>
  {:else}
    <p class="muted">No music folders yet.</p>
  {/each}
  {#if error}<p class="error">{error}</p>{/if}
  <button onclick={add}>Add folder...</button>
  <p class="muted small">
    Files are indexed where they are and are never moved, renamed or edited. Removing a folder keeps its tracks,
    ratings and playlists.
  </p>
</div>

<style>
  .row {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 4px 0;
  }
  .path {
    flex: 1;
    word-break: break-all;
  }
  .small {
    font-size: 12px;
  }
</style>
