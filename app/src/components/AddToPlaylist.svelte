<script lang="ts">
  import { onMount } from 'svelte'
  import { api, type Playlist } from '../lib/api'

  let { trackIds, onadded }: { trackIds: string[]; onadded?: (name: string) => void } = $props()

  let playlists: Playlist[] = $state([])
  let choice = $state('')
  let newName = $state('')
  let creating = $state(false)
  let message: string | null = $state(null)
  let select: HTMLSelectElement | undefined = $state()

  onMount(async () => {
    playlists = await api.playlists()
  })

  export function focus() {
    select?.focus()
  }

  async function add(id: string, name: string) {
    try {
      await api.addToPlaylist(id, trackIds)
      message = `Added to ${name}`
      onadded?.(name)
    } catch (e) {
      message = String(e)
    }
    choice = ''
  }

  async function onchoose() {
    if (choice === '__new') {
      creating = true
      return
    }
    const p = playlists.find((x) => x.id === choice)
    if (p) await add(p.id, p.name)
  }

  async function create() {
    if (!newName.trim()) return
    const id = await api.createPlaylist(newName)
    const name = newName.trim()
    playlists = await api.playlists()
    creating = false
    newName = ''
    await add(id, name)
  }
</script>

<div class="add">
  {#if creating}
    <!-- svelte-ignore a11y_autofocus -->
    <input
      placeholder="Playlist name"
      bind:value={newName}
      autofocus
      onkeydown={(e) => {
        e.stopPropagation()
        if (e.key === 'Enter') create()
        if (e.key === 'Escape') creating = false
      }}
    />
    <button onclick={create}>Create</button>
  {:else}
    <select bind:this={select} bind:value={choice} onchange={onchoose} aria-label="Add to playlist">
      <option value="">Add to playlist...</option>
      {#each playlists as p (p.id)}
        <option value={p.id}>{p.name}</option>
      {/each}
      <option value="__new">New playlist...</option>
    </select>
  {/if}
  {#if message}<span class="muted small">{message}</span>{/if}
</div>

<style>
  .add {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
  }
  .small {
    font-size: 12px;
  }
</style>
