<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { open } from '@tauri-apps/plugin-dialog'
  import { api, type Availability, type LibraryQuery, type LibraryRoot, type LibraryRow, type RatingFilter, type RatingKind, type SortBy } from '../lib/api'
  import { formatDuration, trackLabel } from '../lib/format'
  import { playTrack } from '../lib/player.svelte'
  import TrackPanel from '../components/TrackPanel.svelte'
  import RatingControl from '../components/RatingControl.svelte'
  import Duplicates from '../components/Duplicates.svelte'
  import MetadataCheck from '../components/MetadataCheck.svelte'

  const PAGE = 300

  let roots: LibraryRoot[] = $state([])
  let rows: LibraryRow[] = $state([])
  let total = $state(0)
  let selected: string | null = $state(null)
  let error: string | null = $state(null)
  let notice: string | null = $state(null)
  let showDuplicates = $state(false)
  let searchInput: HTMLInputElement | undefined = $state()

  let text = $state('')
  let rating = $state('')
  let tempoMin = $state('')
  let tempoMax = $state('')
  let key = $state('')
  let availability = $state('')
  let sort: SortBy = $state('artist')
  let descending = $state(false)

  function ratingFilter(v: string): RatingFilter | null {
    if (v === 'unrated' || v === 'thumbs_down') return v
    if (v.startsWith('min')) return { min_stars: Number(v.slice(3)) }
    return null
  }

  function query(offset: number): LibraryQuery {
    const num = (v: string) => (v.trim() === '' ? null : Number(v))
    return {
      text: text || null,
      rating: ratingFilter(rating),
      tempo_min: num(tempoMin),
      tempo_max: num(tempoMax),
      key: key || null,
      availability: (availability || null) as Availability | null,
      sort,
      descending,
      limit: PAGE,
      offset,
    }
  }

  async function load() {
    try {
      const page = await api.search(query(0))
      rows = page.rows
      total = page.total
      error = null
    } catch (e) {
      error = String(e)
    }
  }

  async function loadMore() {
    const page = await api.search(query(rows.length))
    rows = [...rows, ...page.rows]
  }

  let debounce: ReturnType<typeof setTimeout> | undefined
  function scheduleLoad() {
    clearTimeout(debounce)
    debounce = setTimeout(load, 150)
  }

  function sortBy(s: SortBy) {
    if (sort === s) descending = !descending
    else {
      sort = s
      descending = false
    }
    load()
  }

  async function addFolder() {
    const dir = await open({ directory: true, multiple: false, title: 'Choose a music folder' })
    if (typeof dir !== 'string') return
    try {
      await api.addRoot(dir)
      roots = await api.roots()
      notice = 'Indexing started. Files stay where they are; progress is in Activity.'
    } catch (e) {
      error = String(e)
    }
  }

  async function checkFiles() {
    notice = 'Checking files...'
    try {
      const s = await api.checkFiles()
      notice = `Checked ${s.processed} files: ${s.updated} changed, ${s.marked_missing} newly missing.`
      load()
    } catch (e) {
      error = String(e)
    }
  }

  async function rescan() {
    await api.rescan()
    notice = 'Rescan queued. Progress is in Activity.'
  }

  // Refresh while an import is running so new tracks appear.
  let timer: ReturnType<typeof setInterval> | undefined
  onMount(async () => {
    roots = await api.roots()
    load()
    timer = setInterval(() => {
      if (!selected) load()
    }, 5000)
  })
  onDestroy(() => clearInterval(timer))

  function play(row: LibraryRow) {
    playTrack(row.track_id, trackLabel(row))
  }

  function onKey(e: KeyboardEvent) {
    const target = e.target as HTMLElement
    if (target.tagName === 'INPUT' || target.tagName === 'SELECT' || target.tagName === 'TEXTAREA') return
    if (e.key === '/') {
      e.preventDefault()
      searchInput?.focus()
    } else if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && rows.length) {
      e.preventDefault()
      const i = rows.findIndex((r) => r.track_id === selected)
      const next = e.key === 'ArrowDown' ? Math.min(rows.length - 1, i + 1) : Math.max(0, i - 1)
      selected = rows[next].track_id
    } else if (e.key === 'Escape') {
      selected = null
    } else if (e.key === 'Enter' && selected) {
      const row = rows.find((r) => r.track_id === selected)
      if (row) play(row)
    } else if (selected && ['0', '1', '2', '3', 'Backspace'].includes(e.key) && !e.metaKey && !e.ctrlKey) {
      e.preventDefault()
      const row = rows.find((r) => r.track_id === selected)
      if (row) void rate(row, e.key)
    }
  }

  const KEY_RATINGS: Record<string, RatingKind | null> = {
    '0': 'thumbs_down',
    '1': 'star1',
    '2': 'star2',
    '3': 'star3',
    Backspace: null,
  }

  async function rate(row: LibraryRow, key: string) {
    const kind = KEY_RATINGS[key] ?? null
    try {
      await api.libraryRate(row.track_id, kind)
      row.rating = kind
    } catch (e) {
      error = String(e)
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="layout" class:with-panel={selected}>
  <section class="list">
    <header class="bar">
      <input
        bind:this={searchInput}
        class="search"
        type="search"
        placeholder="Search artist, title, mix, label (press /)"
        bind:value={text}
        oninput={scheduleLoad}
      />
      <button onclick={addFolder}>Add folder</button>
      <button onclick={rescan} disabled={roots.length === 0}>Rescan</button>
      <button onclick={checkFiles}>Check files</button>
      <button onclick={() => (showDuplicates = true)}>Duplicates</button>
      <MetadataCheck onchange={load} />
    </header>

    <div class="filters">
      <label>Rating
        <select bind:value={rating} onchange={load}>
          <option value="">Any</option>
          <option value="unrated">Unrated</option>
          <option value="thumbs_down">Thumbs down</option>
          <option value="min1">★ or more</option>
          <option value="min2">★★ or more</option>
          <option value="min3">★★★</option>
        </select>
      </label>
      <label>BPM <input class="num" type="number" placeholder="min" bind:value={tempoMin} oninput={scheduleLoad} />
        to <input class="num" type="number" placeholder="max" bind:value={tempoMax} oninput={scheduleLoad} /></label>
      <label>Key <input class="num" placeholder="any" bind:value={key} oninput={scheduleLoad} /></label>
      <label>Files
        <select bind:value={availability} onchange={load}>
          <option value="">All</option>
          <option value="available">Available</option>
          <option value="missing">Missing</option>
          <option value="corrupt">Unreadable</option>
        </select>
      </label>
      <span class="muted count">{total} tracks</span>
    </div>

    {#if error}<p class="error">{error}</p>{/if}
    {#if notice}<p class="notice">{notice}</p>{/if}

    {#if roots.length === 0 && total === 0}
      <div class="empty">
        <h2>Add your music</h2>
        <p>Choose a folder of music. Crate Digger indexes files where they are and never moves, renames or edits them.</p>
        <button class="primary" onclick={addFolder}>Add folder</button>
      </div>
    {:else}
      <table>
        <thead>
          <tr>
            <th><button class="th" onclick={() => sortBy('artist')}>Artist</button></th>
            <th><button class="th" onclick={() => sortBy('title')}>Title</button></th>
            <th>Mix</th>
            <th>Label</th>
            <th class="r"><button class="th" onclick={() => sortBy('tempo')}>BPM</button></th>
            <th><button class="th" onclick={() => sortBy('key')}>Key</button></th>
            <th class="r">Time</th>
            <th><button class="th" onclick={() => sortBy('rating')}>Rating</button></th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each rows as row (row.track_id)}
            <tr
              class:selected={selected === row.track_id}
              class:unavailable={row.availability !== 'available'}
              onclick={() => (selected = row.track_id)}
              ondblclick={() => play(row)}
            >
              <td>{row.artist ?? ''}</td>
              <td>{row.title ?? row.path?.split(/[\\/]/).pop() ?? ''}</td>
              <td class="muted">{row.mix ?? ''}</td>
              <td class="muted">{row.label ?? ''}</td>
              <td class="r">{row.tempo ?? ''}</td>
              <td>{row.musical_key ?? ''}</td>
              <td class="r muted">{formatDuration(row.duration_ms)}</td>
              <td class="rating">
                <RatingControl trackId={row.track_id} rating={row.rating} onchange={(r) => (row.rating = r)} />
              </td>
              <td class="status" title={row.availability_reason ?? ''}>
                {#if row.availability === 'missing'}Missing{:else if row.availability === 'corrupt'}Unreadable{/if}
                {#if row.playlist_count > 0}<span class="muted">· {row.playlist_count} playlist{row.playlist_count > 1 ? 's' : ''}</span>{/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if rows.length < total}
        <button class="more" onclick={loadMore}>Show more ({total - rows.length} left)</button>
      {/if}
    {/if}
  </section>

  {#if selected}
    <TrackPanel trackId={selected} onclose={() => (selected = null)} onchange={load} />
  {/if}
</div>

{#if showDuplicates}
  <Duplicates onclose={() => ((showDuplicates = false), load())} />
{/if}

<style>
  .layout {
    display: grid;
    grid-template-columns: 1fr;
    gap: 16px;
    height: 100%;
  }
  .layout.with-panel {
    grid-template-columns: 1fr 380px;
  }
  .list {
    min-width: 0;
  }
  .bar {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .search {
    flex: 1;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
    align-items: center;
    margin: 12px 0;
    color: var(--muted);
  }
  .filters label {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .num {
    width: 64px;
  }
  .count {
    margin-left: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
  }
  th,
  td {
    text-align: left;
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  th:nth-child(5),
  th:nth-child(6),
  th:nth-child(7) {
    width: 64px;
  }
  th:nth-child(8) {
    width: 80px;
  }
  th {
    color: var(--muted);
    font-weight: 600;
  }
  .th {
    background: none;
    border: none;
    padding: 0;
    color: var(--muted);
    font-weight: 600;
  }
  .r {
    text-align: right;
  }
  tbody tr {
    cursor: pointer;
  }
  tbody tr:hover {
    background: var(--panel);
  }
  tr.selected {
    background: var(--panel-2) !important;
  }
  tr.unavailable td {
    color: var(--muted);
  }
  .status {
    color: var(--danger);
  }
  .rating {
    color: var(--accent);
  }
  .empty {
    margin: 80px auto;
    max-width: 420px;
    text-align: center;
  }
  .notice {
    color: var(--muted);
  }
  .more {
    margin: 12px 0;
  }
</style>
