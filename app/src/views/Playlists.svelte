<script lang="ts">
  import { onMount } from 'svelte'
  import { api, type Playlist, type PlaylistEntry } from '../lib/api'
  import { formatDuration, formatRating, trackLabel } from '../lib/format'
  import { playTrack } from '../lib/player.svelte'
  import ExportPlaylist from '../components/ExportPlaylist.svelte'

  let playlists: Playlist[] = $state([])
  let current: string | null = $state(null)
  let entries: PlaylistEntry[] = $state([])
  let selectedPos: number | null = $state(null)
  let error: string | null = $state(null)
  let newName = $state('')
  let renaming = $state(false)
  let renameValue = $state('')
  let confirmDelete = $state(false)
  let dragFrom: number | null = $state(null)
  let dropAt: number | null = $state(null)

  const currentPlaylist = $derived(playlists.find((p) => p.id === current) ?? null)

  async function loadPlaylists() {
    playlists = await api.playlists()
    if (!current && playlists.length) current = playlists[0].id
  }

  async function loadEntries() {
    if (!current) {
      entries = []
      return
    }
    entries = await api.playlistEntries(current)
  }

  async function run(fn: () => Promise<unknown>) {
    try {
      await fn()
      error = null
    } catch (e) {
      error = String(e)
    }
    await loadPlaylists()
    await loadEntries()
  }

  async function create() {
    if (!newName.trim()) return
    await run(async () => {
      current = await api.createPlaylist(newName)
      newName = ''
    })
  }

  function select(id: string) {
    current = id
    selectedPos = null
    renaming = false
    confirmDelete = false
    loadEntries()
  }

  async function move(from: number, to: number) {
    if (!current || from === to || to < 0 || to >= entries.length) return
    await run(() => api.movePlaylistEntry(current!, from, to))
    selectedPos = to
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement
    if (['INPUT', 'SELECT', 'TEXTAREA'].includes(t.tagName) || selectedPos == null) return
    if (e.altKey && e.key === 'ArrowUp') {
      e.preventDefault()
      move(selectedPos, selectedPos - 1)
    } else if (e.altKey && e.key === 'ArrowDown') {
      e.preventDefault()
      move(selectedPos, selectedPos + 1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      selectedPos = Math.max(0, selectedPos - 1)
    } else if (e.key === 'ArrowDown') {
      e.preventDefault()
      selectedPos = Math.min(entries.length - 1, selectedPos + 1)
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault()
      run(() => api.removeFromPlaylist(current!, selectedPos!))
    } else if (e.key === 'Enter') {
      const entry = entries[selectedPos]
      if (entry) playTrack(entry.track_id, trackLabel(entry))
    }
  }

  onMount(async () => {
    await loadPlaylists()
    await loadEntries()
  })
</script>

<svelte:window onkeydown={onKey} />

<div class="layout">
  <aside>
    <form class="new" onsubmit={(e) => (e.preventDefault(), create())}>
      <input placeholder="New playlist" bind:value={newName} />
      <button type="submit">Create</button>
    </form>
    {#each playlists as p (p.id)}
      <button class="pl" class:active={p.id === current} onclick={() => select(p.id)}>
        <span>{p.name}</span>
        <span class="muted">{p.track_count}</span>
      </button>
    {/each}
    {#if playlists.length === 0}
      <p class="muted">No playlists yet.</p>
    {/if}
  </aside>

  <section>
    {#if error}<p class="error">{error}</p>{/if}
    {#if currentPlaylist}
      <header>
        {#if renaming}
          <form onsubmit={(e) => (e.preventDefault(), run(() => api.renamePlaylist(current!, renameValue)).then(() => (renaming = false)))}>
            <input bind:value={renameValue} />
            <button type="submit">Save</button>
            <button type="button" onclick={() => (renaming = false)}>Cancel</button>
          </form>
        {:else}
          <h2>{currentPlaylist.name}</h2>
          <span class="muted">{currentPlaylist.track_count} tracks · {formatDuration(currentPlaylist.duration_ms)}</span>
          <div class="actions">
            <button onclick={() => ((renaming = true), (renameValue = currentPlaylist.name))}>Rename</button>
            {#key current}<ExportPlaylist id={current!} />{/key}
            {#if confirmDelete}
              <span class="muted">Delete the playlist? Tracks and files are kept.</span>
              <button class="danger" onclick={() => run(() => api.deletePlaylist(current!)).then(() => ((current = null), (confirmDelete = false), loadPlaylists().then(loadEntries)))}>Delete</button>
              <button onclick={() => (confirmDelete = false)}>Cancel</button>
            {:else}
              <button onclick={() => (confirmDelete = true)}>Delete</button>
            {/if}
          </div>
        {/if}
      </header>

      {#if entries.length === 0}
        <p class="muted">Empty. Add tracks from the Library or the Review queue.</p>
      {:else}
        <p class="muted small">Drag to reorder, or select a row and press Alt+Up / Alt+Down. Delete removes the entry, not the file.</p>
        <table>
          <tbody>
            {#each entries as e, i (e.position + ':' + e.track_id)}
              <tr
                draggable="true"
                class:selected={selectedPos === i}
                class:drop-above={dropAt === i && dragFrom !== null && dragFrom > i}
                class:drop-below={dropAt === i && dragFrom !== null && dragFrom < i}
                class:unavailable={e.availability !== 'available'}
                onclick={() => (selectedPos = i)}
                ondblclick={() => playTrack(e.track_id, trackLabel(e))}
                ondragstart={() => (dragFrom = i)}
                ondragover={(ev) => (ev.preventDefault(), (dropAt = i))}
                ondrop={(ev) => {
                  ev.preventDefault()
                  if (dragFrom !== null) move(dragFrom, i)
                  dragFrom = null
                  dropAt = null
                }}
                ondragend={() => ((dragFrom = null), (dropAt = null))}
              >
                <td class="n muted">{i + 1}</td>
                <td>{trackLabel(e)}</td>
                <td class="muted">{e.tempo ?? ''}</td>
                <td class="muted">{e.musical_key ?? ''}</td>
                <td class="muted">{formatDuration(e.duration_ms)}</td>
                <td class="rating">{formatRating(e.rating)}</td>
                <td class="status">{e.availability === 'missing' ? 'Missing' : e.availability === 'corrupt' ? 'Unreadable' : ''}</td>
                <td><button class="x" onclick={(ev) => (ev.stopPropagation(), run(() => api.removeFromPlaylist(current!, i)))} aria-label="Remove from playlist">Remove</button></td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {:else}
      <p class="muted">Create a playlist to start.</p>
    {/if}
  </section>
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: 240px 1fr;
    gap: 20px;
  }
  aside {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .new {
    display: flex;
    gap: 6px;
    margin-bottom: 8px;
  }
  .new input {
    flex: 1;
    min-width: 0;
  }
  .pl {
    display: flex;
    justify-content: space-between;
    background: transparent;
    border-color: transparent;
    text-align: left;
  }
  .pl.active {
    background: var(--panel-2);
    border-color: var(--border);
  }
  header {
    display: flex;
    gap: 12px;
    align-items: baseline;
    flex-wrap: wrap;
  }
  header h2 {
    margin: 0;
  }
  .actions {
    margin-left: auto;
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .small {
    font-size: 12px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
  }
  tr {
    cursor: grab;
  }
  tr.selected {
    background: var(--panel-2);
  }
  tr.drop-above td {
    border-top: 2px solid var(--accent);
  }
  tr.drop-below td {
    border-bottom: 2px solid var(--accent);
  }
  tr.unavailable td {
    color: var(--muted);
  }
  .n {
    width: 32px;
  }
  .rating {
    color: var(--accent);
  }
  .status {
    color: var(--danger);
  }
  .x {
    padding: 2px 8px;
    font-size: 12px;
  }
  .danger {
    border-color: var(--danger);
    color: var(--danger);
  }
</style>
