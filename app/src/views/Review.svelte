<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { openUrl } from '@tauri-apps/plugin-opener'
  import { api, type QueueStats, type RatingKind, type ReviewCard } from '../lib/api'
  import { formatCodec, formatDuration, trackLabel } from '../lib/format'
  import { player, playTrack, seekBy, seekTo } from '../lib/player.svelte'
  import AddToPlaylist from '../components/AddToPlaylist.svelte'
  import DiscoverFrom from '../components/DiscoverFrom.svelte'
  import DownloadChoices from '../components/DownloadChoices.svelte'
  import QueueHealth from '../components/QueueHealth.svelte'
  import YoutubeLinks from '../components/YoutubeLinks.svelte'
  import Waveform from '../components/Waveform.svelte'

  let cards: ReviewCard[] = $state([])
  let stats: QueueStats | null = $state(null)
  let error: string | null = $state(null)
  let flash: string | null = $state(null)
  // Actions run one after another in the order pressed, so quick key
  // presses are never dropped. A card that has already been rated or
  // skipped ignores further ratings until the queue reloads.
  let chain: Promise<void> = Promise.resolve()
  const actedOn = new Set<string>()
  let demo = $state(false)
  let pinned: string | null = null
  let addToPlaylist: AddToPlaylist | undefined = $state()

  const card = $derived(cards[0] ?? null)

  async function load() {
    try {
      let next = await api.reviewNext(5)
      // After an undo, show the restored track first.
      if (pinned) {
        const i = next.findIndex((c) => c.track_id === pinned)
        if (i > 0) next = [next[i], ...next.slice(0, i), ...next.slice(i + 1)]
        pinned = null
      }
      cards = next
      stats = await api.reviewStats()
    } catch (e) {
      error = String(e)
    }
  }

  // Start the current card playing when it changes.
  let autoplayed: string | null = null
  $effect(() => {
    if (card && card.track_id !== autoplayed) {
      autoplayed = card.track_id
      if (player.trackId !== card.track_id) playTrack(card.track_id, trackLabel(card.meta))
    }
  })

  function say(msg: string) {
    flash = msg
    setTimeout(() => {
      if (flash === msg) flash = null
    }, 1800)
  }

  function act(fn: () => Promise<unknown>, msg: string) {
    chain = chain.then(async () => {
      try {
        await fn()
        error = null
        say(msg)
      } catch (e) {
        error = String(e)
      }
      await load()
      actedOn.clear()
    })
  }

  /** Run a card-consuming action once per card. */
  function once(trackId: string, fn: () => Promise<unknown>, msg: string) {
    if (actedOn.has(trackId)) return
    actedOn.add(trackId)
    act(fn, msg)
  }

  const labels: Record<RatingKind, string> = {
    thumbs_down: 'Thumbs down',
    star1: 'One star',
    star2: 'Two stars',
    star3: 'Three stars',
  }

  function rate(kind: RatingKind) {
    if (!card) return
    const id = card.track_id
    once(id, () => api.rate(id, kind), labels[kind])
  }

  function skip() {
    if (!card) return
    const id = card.track_id
    once(id, () => api.skip(id), 'Skipped for this session')
  }

  function undo() {
    act(async () => {
      const u = await api.undo()
      if (!u) throw new Error('Nothing to undo in this session')
      pinned = u.track_id
    }, 'Undone')
  }

  function toggleKeep() {
    if (!card) return
    const id = card.track_id
    const keep = !card.kept
    act(() => api.keep(id, keep), keep ? 'Kept' : 'No longer kept')
  }

  async function findMore() {
    try {
      await api.findMore()
      say('Looking for more tracks. They appear here once audio is ready.')
    } catch (e) {
      error = String(e)
    }
  }

  async function enableDemo() {
    await api.setDemoDiscovery(true)
    demo = true
    await findMore()
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement
    if (['INPUT', 'SELECT', 'TEXTAREA'].includes(t.tagName) || e.metaKey || e.ctrlKey || e.altKey) return
    // Shortcuts belong to the card, not to a dialog shown over it.
    if (document.querySelector('[role="dialog"]')) return
    const k = e.key.toLowerCase()
    const map: Record<string, () => void> = {
      '0': () => rate('thumbs_down'),
      '1': () => rate('star1'),
      '2': () => rate('star2'),
      '3': () => rate('star3'),
      s: skip,
      z: undo,
      k: toggleKeep,
      p: () => addToPlaylist?.focus(),
      arrowleft: () => seekBy(-10_000),
      arrowright: () => seekBy(10_000),
    }
    const fn = map[k]
    if (fn && !e.shiftKey) {
      e.preventDefault()
      fn()
    }
  }

  // Swipe: drag the card left for thumbs down, right for one star.
  let dragX = $state(0)
  let dragStart: number | null = null
  function pointerDown(e: PointerEvent) {
    if ((e.target as HTMLElement).closest('button, a, select, input, canvas')) return
    dragStart = e.clientX
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  }
  function pointerMove(e: PointerEvent) {
    if (dragStart !== null) dragX = e.clientX - dragStart
  }
  function pointerUp() {
    if (dragStart === null) return
    if (dragX < -140) rate('thumbs_down')
    else if (dragX > 140) rate('star1')
    dragStart = null
    dragX = 0
  }

  let timer: ReturnType<typeof setInterval> | undefined
  onMount(async () => {
    demo = await api.demoDiscovery()
    await load()
    timer = setInterval(() => {
      if (!card) load()
      else api.reviewStats().then((s) => (stats = s))
    }, 3000)
  })
  onDestroy(() => clearInterval(timer))
</script>

<svelte:window onkeydown={onKey} />

<section class="review">
  <header>
    <h2>Review</h2>
    {#if stats}
      <span class="muted">
        {stats.ready} ready · {stats.in_progress} on the way
        {#if stats.skipped}· {stats.skipped} skipped this session{/if}
        {#if stats.needs_review}· {stats.needs_review} need a decision{/if}
        {#if stats.failed}· {stats.failed} failed{/if}
      </span>
    {/if}
    <button class="more" onclick={findMore}>Find more</button>
  </header>

  {#if error}<p class="error">{error}</p>{/if}
  <QueueHealth />
  <DownloadChoices />

  {#if card}
    <div
      class="card"
      role="group"
      aria-label="Track to review"
      style:transform="translateX({dragX}px) rotate({dragX / 40}deg)"
      class:swipe-left={dragX < -140}
      class:swipe-right={dragX > 140}
      onpointerdown={pointerDown}
      onpointermove={pointerMove}
      onpointerup={pointerUp}
      onpointercancel={pointerUp}
    >
      <div class="headline">
        <div class="art" aria-hidden="true">{(card.meta.artist ?? '?').slice(0, 1)}</div>
        <div>
          <h1>{card.meta.title ?? 'Untitled'}{#if card.meta.mix}{' '}<span class="mix">({card.meta.mix})</span>{/if}</h1>
          <div class="artist">{card.meta.artist ?? 'Unknown artist'}</div>
          <div class="muted">
            {[card.meta.label, card.meta.release, card.meta.year].filter(Boolean).join(' · ')}
          </div>
        </div>
      </div>

      <Waveform peaks={player.trackId === card.track_id ? player.waveform : null} positionMs={player.trackId === card.track_id ? player.positionMs : 0} durationMs={card.file.duration_ms} onseek={seekTo} height={72} />
      <div class="muted small file">
        {formatCodec(card.file.format)} · {formatDuration(card.file.duration_ms)}
        {#if card.file.bitrate_kbps}· {card.file.bitrate_kbps} kbps{/if}
        {#if card.meta.tempo}· {card.meta.tempo} BPM{/if}
        {#if card.meta.musical_key}· {card.meta.musical_key}{/if}
      </div>

      <div class="actions" role="toolbar" aria-label="Rate">
        <button onclick={() => rate('thumbs_down')} title="Not for me (0)">Thumbs down <kbd>0</kbd></button>
        <button onclick={() => rate('star1')} title="Some interest (1)">★ <kbd>1</kbd></button>
        <button onclick={() => rate('star2')} title="Strong interest (2)">★★ <kbd>2</kbd></button>
        <button onclick={() => rate('star3')} title="Favourite (3)">★★★ <kbd>3</kbd></button>
        <span class="sep"></span>
        <button onclick={skip}>Skip <kbd>S</kbd></button>
        <button onclick={undo}>Undo <kbd>Z</kbd></button>
        <button onclick={toggleKeep} class:on={card.kept}>{card.kept ? 'Kept' : 'Keep'} <kbd>K</kbd></button>
      </div>
      <div class="playlist-row">
        {#key card.track_id}
          <AddToPlaylist bind:this={addToPlaylist} trackIds={[card.track_id]} onadded={(n) => say(`Added to ${n}`)} />
        {/key}
        <span class="muted small">Space plays or pauses · arrows seek · swipe left for thumbs down, right for one star</span>
      </div>

      <div class="details">
        <div>
          <h4>Why this track</h4>
          {#each card.reasons as r}<p>{r}</p>{:else}<p class="muted">No reason recorded.</p>{/each}
          {#if card.confidence != null}<p class="muted small">Match confidence {Math.round(card.confidence * 100)}%{card.verified ? '' : ' · unverified'}</p>{/if}
        </div>
        <div>
          <h4>Evidence</h4>
          {#each card.evidence as ev}
            <div class="evidence">
              <div>"{ev.excerpt}"</div>
              <div class="muted small">
                {ev.source_kind}
                {#if ev.source_url}· <button class="link" onclick={() => openUrl(ev.source_url!)}>{ev.source_url}</button>{/if}
                · retrieved {new Date(ev.retrieved_at).toLocaleString()}
              </div>
            </div>
          {:else}
            <p class="muted">No evidence recorded.</p>
          {/each}
        </div>
        <div>
          <h4>YouTube</h4>
          {#key card.track_id}
            <YoutubeLinks trackId={card.track_id} links={card.youtube} status={card.youtube_status} onchanged={load} />
          {/key}
        </div>
      </div>
    </div>
    {#if flash}<div class="flash" role="status">{flash}</div>{/if}
  {:else}
    <div class="empty">
      {#if flash}<p role="status">{flash}</p>{/if}
      {#if stats && stats.in_progress > 0}
        <h3>Getting tracks ready</h3>
        <p class="muted">{stats.in_progress} tracks are being fetched and checked. They appear here when their audio is playable.</p>
      {:else}
        <h3>Nothing to review</h3>
        {#if demo}
          <p class="muted">Ask for more demo tracks.</p>
          <button class="primary" onclick={findMore}>Find more</button>
        {:else}
          <p class="muted">
            Find more expands your seeds and ratings through Discogs or your model (set them up in Settings). You can
            also read a public tracklist page or paste text. Soulseek downloads arrive in a later release; until then
            new tracks wait in the queue with a reason.
          </p>
          <button class="primary" onclick={findMore}>Find more</button>
          <p class="muted small">Or try demo discovery, which generates tone recordings. Nothing is downloaded.</p>
          <button onclick={enableDemo}>Try demo discovery</button>
        {/if}
      {/if}
      {#if !demo}<DiscoverFrom />{/if}
      {#if stats?.skipped}<p class="muted">{stats.skipped} skipped tracks come back next session.</p>{/if}
      <button onclick={undo}>Undo last <kbd>Z</kbd></button>
    </div>
  {/if}
</section>

<style>
  .review {
    max-width: 900px;
    margin: 0 auto;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: 12px;
  }
  header .more {
    margin-left: auto;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 20px 24px;
    margin-top: 12px;
    touch-action: pan-y;
    user-select: none;
    transition: border-color 0.1s;
  }
  .card.swipe-left {
    border-color: var(--danger);
  }
  .card.swipe-right {
    border-color: var(--accent);
  }
  .headline {
    display: flex;
    gap: 16px;
    align-items: center;
    margin-bottom: 16px;
  }
  .art {
    width: 72px;
    height: 72px;
    border-radius: 8px;
    background: var(--panel-2);
    display: grid;
    place-items: center;
    font-size: 32px;
    color: var(--muted);
    flex: none;
  }
  h1 {
    margin: 0;
    font-size: 24px;
  }
  .mix {
    color: var(--muted);
    font-weight: 400;
  }
  .artist {
    font-size: 17px;
  }
  .file {
    margin: 6px 0 14px;
  }
  .small {
    font-size: 12px;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }
  .actions .sep {
    width: 12px;
  }
  .actions .on {
    border-color: var(--accent);
    color: var(--accent);
  }
  kbd {
    font-size: 11px;
    color: var(--muted);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 4px;
    margin-left: 4px;
  }
  .playlist-row {
    display: flex;
    gap: 16px;
    align-items: center;
    margin: 12px 0 4px;
    flex-wrap: wrap;
  }
  .details {
    display: grid;
    grid-template-columns: 1fr 1.4fr 1fr;
    gap: 20px;
    border-top: 1px solid var(--border);
    margin-top: 16px;
    padding-top: 8px;
  }
  .details p {
    margin: 4px 0;
  }
  .evidence {
    margin-bottom: 8px;
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    text-align: left;
    word-break: break-all;
  }
  .flash {
    position: fixed;
    bottom: 90px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--panel-2);
    border: 1px solid var(--border);
    padding: 8px 16px;
    border-radius: 8px;
  }
  .empty {
    text-align: center;
    margin: 80px auto;
    max-width: 480px;
  }
</style>
