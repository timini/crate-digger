<script module lang="ts">
  // Kept while the app is open, so leaving and returning keeps the view.
  const saved = {
    viewport: null as { cx: number; cy: number; scale: number } | null,
    selected: [] as string[],
    mode: 'cluster' as import('../lib/mapColour').ColourMode,
    graph: false,
  }
</script>

<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import {
    api, connections, libraryMap, type MapNeighbour, type MapParams, type MapPoint, type MapView,
    type Playlist, type Unplaced,
  } from '../lib/api'
  import { colouring, yearColour, type ColourMode } from '../lib/mapColour'
  import { playTrack } from '../lib/player.svelte'
  import { formatRelative, trackLabel } from '../lib/format'
  import AddToPlaylist from '../components/AddToPlaylist.svelte'

  let view = $state<MapView | null>(null)
  let error = $state('')
  let message = $state('')
  let mode: ColourMode = $state(saved.mode)
  let graph = $state(saved.graph)
  let graphNeighbours = $state(5)
  let highlight = $state(new Set<string>())
  let selected: string[] = $state([...saved.selected])
  let neighbours: MapNeighbour[] = $state([])
  let unplaced: Unplaced[] | null = $state(null)
  let playlists: Playlist[] = $state([])
  let edges: [string, string, number][] = $state([])
  let search = $state('')
  let params: MapParams = $state({ neighbours: 10, min_samples: 5, eps: null, epochs: 200, seed: 42 })
  let autoEps = $state(true)
  let canvas: HTMLCanvasElement | undefined = $state()
  let width = $state(800)
  let height = $state(520)
  let viewport = $state(saved.viewport ?? { cx: 0, cy: 0, scale: 1 })
  let timer: ReturnType<typeof setInterval> | undefined

  const points = $derived(view?.points ?? [])
  const byId = $derived(new Map(points.map((p) => [p.track_id, p])))
  const playlistNames = $derived(new Map(playlists.map((p) => [p.id, p.name])))
  const colours = $derived(colouring(points, mode, highlight, playlistNames))
  const focus = $derived(selected.length ? byId.get(selected[selected.length - 1]) ?? null : null)
  const matches = $derived.by(() => {
    const q = search.trim().toLowerCase()
    if (!q) return []
    return points.filter((p) => trackLabel(p).toLowerCase().includes(q)).slice(0, 50)
  })

  $effect(() => { saved.mode = mode; saved.graph = graph; saved.selected = [...selected]; saved.viewport = { ...viewport } })

  async function load() {
    try {
      const had = view?.map?.id
      view = await libraryMap.get()
      if (view.map && view.map.id !== had) {
        if (!saved.viewport || had) fit()
        if (graph) edges = await libraryMap.edges(graphNeighbours)
        selected = selected.filter((id) => view!.points.some((p) => p.track_id === id))
      }
      if (view.building && !timer) timer = setInterval(load, 1000)
      if (!view.building && timer) { clearInterval(timer); timer = undefined }
    } catch (e) { error = String(e) }
  }

  function fit() {
    if (!points.length) return
    const xs = points.map((p) => p.x), ys = points.map((p) => p.y)
    const [minX, maxX, minY, maxY] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)]
    const span = Math.max(maxX - minX, maxY - minY, 1e-3)
    viewport = { cx: (minX + maxX) / 2, cy: (minY + maxY) / 2, scale: (Math.min(width, height) * 0.9) / span }
  }

  const toScreen = (p: { x: number; y: number }) => [
    (p.x - viewport.cx) * viewport.scale + width / 2,
    (p.y - viewport.cy) * viewport.scale + height / 2,
  ]

  function draw() {
    const ctx = canvas?.getContext('2d')
    if (!ctx || !canvas) return
    const dpr = window.devicePixelRatio || 1
    canvas.width = width * dpr; canvas.height = height * dpr
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.clearRect(0, 0, width, height)
    const chosen = new Set(selected)
    if (graph && edges.length) {
      ctx.lineWidth = 1
      for (const [a, b] of edges) {
        const pa = byId.get(a), pb = byId.get(b)
        if (!pa || !pb) continue
        const near = chosen.has(a) || chosen.has(b)
        ctx.strokeStyle = near ? 'rgba(245, 245, 245, 0.8)' : 'rgba(138, 143, 152, 0.12)'
        const [x1, y1] = toScreen(pa), [x2, y2] = toScreen(pb)
        ctx.beginPath(); ctx.moveTo(x1, y1); ctx.lineTo(x2, y2); ctx.stroke()
      }
    }
    for (const p of points) {
      const [x, y] = toScreen(p)
      if (x < -5 || y < -5 || x > width + 5 || y > height + 5) continue
      ctx.fillStyle = colours.colour(p)
      ctx.beginPath(); ctx.arc(x, y, 3, 0, Math.PI * 2); ctx.fill()
    }
    ctx.lineWidth = 2
    for (const id of selected) {
      const p = byId.get(id)
      if (!p) continue
      const [x, y] = toScreen(p)
      ctx.strokeStyle = id === focus?.track_id ? '#ffffff' : '#edc948'
      ctx.beginPath(); ctx.arc(x, y, 6, 0, Math.PI * 2); ctx.stroke()
    }
    if (box) {
      ctx.strokeStyle = '#edc948'; ctx.lineWidth = 1
      ctx.strokeRect(box.x0, box.y0, box.x1 - box.x0, box.y1 - box.y0)
    }
  }

  $effect(() => {
    // Redraw when anything drawn changes.
    void [points, colours, selected, viewport, width, height, graph, edges, box]
    draw()
  })

  $effect(() => {
    const id = focus?.track_id
    if (!id) { neighbours = []; return }
    libraryMap.neighbours(id).then((n) => { if (focus?.track_id === id) neighbours = n }).catch(() => (neighbours = []))
  })

  async function toggleGraph() {
    graph = !graph
    if (graph) edges = await libraryMap.edges(graphNeighbours).catch(() => [])
  }

  function nearest(sx: number, sy: number): MapPoint | null {
    let best: MapPoint | null = null, bd = 64
    for (const p of points) {
      const [x, y] = toScreen(p)
      const d = (x - sx) ** 2 + (y - sy) ** 2
      if (d < bd) { bd = d; best = p }
    }
    return best
  }

  let drag: { x: number; y: number; cx: number; cy: number; moved: boolean; shift: boolean } | null = null
  let box: { x0: number; y0: number; x1: number; y1: number } | null = $state(null)

  function down(e: PointerEvent) {
    canvas?.setPointerCapture(e.pointerId)
    drag = { x: e.offsetX, y: e.offsetY, cx: viewport.cx, cy: viewport.cy, moved: false, shift: e.shiftKey }
  }
  function move(e: PointerEvent) {
    if (!drag) return
    const dx = e.offsetX - drag.x, dy = e.offsetY - drag.y
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true
    if (drag.shift) {
      box = { x0: Math.min(drag.x, e.offsetX), y0: Math.min(drag.y, e.offsetY), x1: Math.max(drag.x, e.offsetX), y1: Math.max(drag.y, e.offsetY) }
    } else {
      viewport = { ...viewport, cx: drag.cx - dx / viewport.scale, cy: drag.cy - dy / viewport.scale }
    }
  }
  function up(e: PointerEvent) {
    if (!drag) return
    if (box) {
      const inside = points.filter((p) => {
        const [x, y] = toScreen(p)
        return x >= box!.x0 && x <= box!.x1 && y >= box!.y0 && y <= box!.y1
      }).map((p) => p.track_id)
      selected = [...new Set([...selected, ...inside])]
      box = null
    } else if (!drag.moved) {
      const p = nearest(e.offsetX, e.offsetY)
      if (p) select(p.track_id, e.shiftKey)
      else if (!e.shiftKey) selected = []
    }
    drag = null
  }
  function wheel(e: WheelEvent) {
    e.preventDefault()
    const k = Math.exp(-e.deltaY * 0.002)
    const wx = (e.offsetX - width / 2) / viewport.scale + viewport.cx
    const wy = (e.offsetY - height / 2) / viewport.scale + viewport.cy
    const scale = Math.min(Math.max(viewport.scale * k, 0.5), 5000)
    viewport = { scale, cx: wx - (e.offsetX - width / 2) / scale, cy: wy - (e.offsetY - height / 2) / scale }
  }

  function select(id: string, add = false) {
    if (add) selected = selected.includes(id) ? selected.filter((s) => s !== id) : [...selected, id]
    else selected = [id]
  }

  function centreOn(id: string) {
    const p = byId.get(id)
    if (p) viewport = { ...viewport, cx: p.x, cy: p.y }
    select(id)
  }

  function toggleHighlight(key: string) {
    const next = new Set(highlight)
    if (next.has(key)) next.delete(key)
    else next.add(key)
    highlight = next
  }

  async function useAsSeeds() {
    try {
      const seeds = await connections.seeds()
      const have = new Set(seeds.map((s) => `${s.kind}:${s.value.toLowerCase()}`))
      let added = 0
      for (const id of selected) {
        const p = byId.get(id)
        if (!p?.artist || !p.title) continue
        const value = `${p.artist} - ${p.title}${p.mix ? ` (${p.mix})` : ''}`
        if (have.has(`track:${value.toLowerCase()}`)) continue
        seeds.push({ kind: 'track', value }); have.add(`track:${value.toLowerCase()}`); added++
      }
      await connections.saveSeeds(seeds)
      message = added ? `Added ${added} discovery seeds.` : 'Those tracks are already seeds, or have no artist and title.'
    } catch (e) { error = String(e) }
  }

  async function rebuild() {
    error = ''
    try {
      await libraryMap.rebuild({ ...params, eps: autoEps ? null : params.eps })
      await load()
    } catch (e) { error = String(e) }
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === 'Escape') selected = []
  }

  onMount(async () => {
    await load()
    playlists = await api.playlists().catch(() => [])
    if (graph) edges = await libraryMap.edges(graphNeighbours).catch(() => [])
  })
  onDestroy(() => timer && clearInterval(timer))
</script>

<svelte:window onkeydown={keydown} />

<section class="map">
  <header>
    <h2>Library map</h2>
    {#if view}
      <p class="muted">
        {view.coverage.embedded} of {view.coverage.eligible} library tracks have an analysis to place them.
        {#if view.map}
          The map shows {view.map.placed}, in {view.map.clusters} clusters with {view.map.noise} unclustered, made {formatRelative(view.map.created_at)}.
        {/if}
      </p>
    {/if}
  </header>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
  {#if view?.last_error}<p class="error">{view.last_error}</p>{/if}
  {#if view?.coverage.stale}<p class="stale" role="status">{view.coverage.stale}</p>{/if}

  <div class="toolbar">
    {#if view?.building}
      <progress max="100" value={view.building.progress}></progress>
      <span>Building the map ({view.building.progress}%). Playback and the rest of the app keep working.</span>
      <button onclick={() => libraryMap.cancel().then(load)}>Cancel</button>
    {:else}
      <button class="primary" onclick={rebuild} disabled={!view?.coverage.embedded}>{view?.map ? 'Rebuild map' : 'Build map'}</button>
    {/if}
    <details>
      <summary>Map settings</summary>
      <label>Neighbours per track <input type="number" min="2" max="50" bind:value={params.neighbours} /></label>
      <label>Tracks needed to start a cluster <input type="number" min="2" max="50" bind:value={params.min_samples} /></label>
      <label class="check"><input type="checkbox" bind:checked={autoEps} />Choose the cluster distance from the library</label>
      {#if !autoEps}
        <label>Cluster distance (cosine) <input type="number" min="0.001" max="2" step="0.01" bind:value={params.eps} /></label>
      {/if}
      <p class="muted small">Rebuilding may move points and renumber clusters. Clusters are groups found in the analysis, not genres.</p>
    </details>
  </div>

  {#if view?.map}
    <div class="controls">
      <label>Colour by
        <select bind:value={mode} onchange={() => (highlight = new Set())}>
          <option value="cluster">Cluster</option>
          <option value="playlist">Playlist</option>
          <option value="year">Release year</option>
          <option value="genre">Genre</option>
          <option value="label">Label</option>
          <option value="country">Release country</option>
        </select>
      </label>
      <button aria-pressed={graph} onclick={toggleGraph}>{graph ? 'Hide neighbour links' : 'Show neighbour links'}</button>
      {#if graph}
        <label>Links per track
          <input type="range" min="1" max={view.map.provenance.neighbours} bind:value={graphNeighbours}
            onchange={async () => (edges = await libraryMap.edges(graphNeighbours))} />
        </label>
      {/if}
      <button onclick={fit}>Fit</button>
      <label class="search">Find a track <input type="search" bind:value={search} placeholder="Artist or title" /></label>
    </div>
    {#if matches.length}
      <ul class="matches" aria-label="Matching tracks">
        {#each matches as p (p.track_id)}
          <li><button onclick={() => centreOn(p.track_id)}>{trackLabel(p)}</button></li>
        {/each}
      </ul>
    {/if}

    <div class="body">
      <div class="canvas" bind:clientWidth={width} bind:clientHeight={height}>
        <canvas bind:this={canvas} style="width: {width}px; height: {height}px"
          aria-label="Map of library tracks. Use Find a track or the neighbour list to move between tracks."
          onpointerdown={down} onpointermove={move} onpointerup={up} onwheel={wheel}></canvas>
        <p class="hint muted small">Drag to pan, scroll to zoom, click to inspect. Shift-click or shift-drag to select several.</p>
      </div>
      <aside>
        <h3>Legend</h3>
        {#if colours.years}
          <div class="scale" style="background: linear-gradient(to right, {yearColour(colours.years[0], ...colours.years)}, {yearColour(colours.years[1], ...colours.years)})"></div>
          <div class="scale-labels"><span>{colours.years[0]}</span><span>{colours.years[1]}</span></div>
        {/if}
        <ul class="legend">
          {#each colours.legend as l (l.key + l.label)}
            <li>
              <button class:off={highlight.size > 0 && !highlight.has(l.key) && l.key !== '__overlap'} disabled={mode === 'year' || l.key === '__overlap'}
                aria-pressed={highlight.has(l.key)} onclick={() => toggleHighlight(l.key)}>
                <span class="swatch" style="background: {l.colour}"></span>{l.label} <span class="muted">{l.count}</span>
              </button>
            </li>
          {/each}
        </ul>
        {#if mode !== 'year'}<p class="muted small">Click entries to highlight them. Nothing is reclustered.</p>{/if}

        {#if focus}
          <h3>{trackLabel(focus)}</h3>
          <dl>
            <dt>Cluster</dt><dd>{focus.cluster === null ? 'None (noise)' : focus.cluster + 1}</dd>
            <dt>Release year</dt><dd>{focus.year ?? 'Unknown'}</dd>
            <dt>Genre</dt><dd>{focus.genre ?? 'Unknown'}</dd>
            <dt>Label</dt><dd>{focus.label ?? 'Unknown'}</dd>
            <dt>Release country</dt><dd>{focus.release_country ?? 'Unknown'}</dd>
            <dt>Playlists</dt><dd>{focus.playlists.map((id) => playlistNames.get(id) ?? 'Playlist').join(', ') || 'None'}</dd>
          </dl>
          <button onclick={() => playTrack(focus!.track_id, trackLabel(focus!))}>Play</button>
          <h4>Nearest by sound</h4>
          <p class="muted small">Similarity between analyses (1 is identical). Positions on the map are approximate.</p>
          <ol class="neighbours">
            {#each neighbours as n (n.track_id)}
              <li><button onclick={() => centreOn(n.track_id)}>{trackLabel(n)}</button> <span class="muted">{n.similarity.toFixed(3)}</span></li>
            {/each}
          </ol>
        {/if}

        {#if selected.length}
          <h3>{selected.length} selected</h3>
          <AddToPlaylist trackIds={selected} onadded={(name) => (message = `Added ${selected.length} tracks to ${name}.`)} />
          <button onclick={useAsSeeds}>Use as discovery seeds</button>
          <button onclick={() => (selected = [])}>Clear selection</button>
        {/if}
      </aside>
    </div>
  {:else if view && !view.building}
    <p class="muted">No map yet. Build one once some library tracks are analysed.</p>
  {/if}

  {#if view && view.coverage.eligible > view.coverage.embedded}
    <details ontoggle={async (e) => { if (e.currentTarget.open && !unplaced) unplaced = await libraryMap.unplaced() }}>
      <summary>{view.coverage.eligible - view.coverage.embedded} library tracks are not on the map</summary>
      {#if unplaced}
        <ul class="unplaced">
          {#each unplaced as u (u.track_id)}<li>{trackLabel({ ...u, mix: null })} <span class="muted">{u.reason}</span></li>{/each}
        </ul>
      {/if}
    </details>
  {/if}

  {#if view?.map}
    <details>
      <summary>How this map was made</summary>
      <p class="muted small">
        Model {view.map.version.model_id}, pipeline {view.map.provenance.pipeline_version}.
        {view.map.provenance.neighbours} nearest neighbours by {view.map.provenance.distance} distance on {view.map.provenance.preprocessing}.
        Clusters by DBSCAN with a cluster distance of {view.map.provenance.eps.toFixed(3)} ({view.map.provenance.eps_rule})
        and {view.map.provenance.min_samples} tracks to start a cluster. Layout: {view.map.provenance.layout}, {view.map.provenance.epochs} epochs, seed {view.map.provenance.seed}.
        Built in {(view.map.build_ms / 1000).toFixed(1)} s.
      </p>
    </details>
  {/if}
</section>

<style>
  .map { display: grid; gap: 10px; }
  header p { margin: 4px 0 0; }
  .toolbar, .controls { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }
  .toolbar details label, .controls label { display: flex; gap: 6px; align-items: center; }
  .toolbar details { display: grid; gap: 6px; }
  .search input { width: 200px; }
  .matches { list-style: none; padding: 0; margin: 0; max-height: 160px; overflow: auto; }
  .matches button, .neighbours button { background: none; border: none; padding: 2px 0; color: inherit; text-align: left; cursor: pointer; text-decoration: underline; }
  .body { display: grid; grid-template-columns: 1fr 280px; gap: 12px; min-height: 520px; }
  .canvas { position: relative; min-height: 520px; border: 1px solid var(--border); border-radius: 6px; overflow: hidden; }
  canvas { display: block; touch-action: none; cursor: crosshair; }
  .hint { position: absolute; bottom: 4px; left: 8px; margin: 0; }
  aside { overflow: auto; max-height: 720px; }
  aside h3 { margin: 12px 0 6px; font-size: 14px; }
  .legend { list-style: none; padding: 0; margin: 0; display: grid; gap: 2px; }
  .legend button { display: flex; align-items: center; gap: 6px; background: none; border: none; color: inherit; padding: 2px 0; cursor: pointer; text-align: left; }
  .legend button.off { opacity: 0.45; }
  .swatch { width: 12px; height: 12px; border-radius: 50%; flex: none; }
  .scale { height: 10px; border-radius: 4px; }
  .scale-labels { display: flex; justify-content: space-between; font-size: 12px; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 2px 8px; font-size: 13px; }
  dt { color: var(--muted, #8a8f98); }
  dd { margin: 0; }
  .neighbours { padding-left: 20px; font-size: 13px; }
  .unplaced { font-size: 13px; }
  .stale { color: var(--warn, #edc948); }
  .small { font-size: 12px; }
  .check { display: flex; gap: 6px; align-items: center; }
  @media (max-width: 900px) { .body { grid-template-columns: 1fr; } }
</style>
