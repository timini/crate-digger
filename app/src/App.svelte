<script lang="ts">
  import { onMount } from 'svelte'
  import { api, type AppInfo } from './lib/api'
  import Activity from './views/Activity.svelte'
  import Library from './views/Library.svelte'
  import LibraryMap from './views/Map.svelte'
  import PlayerBar from './components/PlayerBar.svelte'
  import Playlists from './views/Playlists.svelte'
  import Review from './views/Review.svelte'
  import Settings from './views/Settings.svelte'
  import Identity from './views/Identity.svelte'
  import Onboarding from './components/Onboarding.svelte'
  import type { AppSettings } from './lib/api'
  import { nav, type View } from './lib/nav.svelte'

  // Playlists come first: they are where discovery starts.
  const views: { id: View; label: string }[] = [
    { id: 'playlists', label: 'Playlists' },
    { id: 'review', label: 'Discovery' },
    { id: 'library', label: 'Library' },
    { id: 'map', label: 'Map' },
    { id: 'activity', label: 'Activity' },
    { id: 'identity', label: 'Identity' },
    { id: 'settings', label: 'Settings' },
  ]

  let info: AppInfo | null = $state(null)
  let error: string | null = $state(null)
  let firstRun: AppSettings | null = $state(null)
  let conflicts = $state(0)

  async function refreshConflicts() {
    try {
      conflicts = await api.conflictCount()
    } catch {
      // Not critical; the badge simply stays as it was.
    }
  }

  onMount(async () => {
    try {
      info = await api.appInfo()
      const s = await api.settings()
      if (!s.onboarded) firstRun = s
      refreshConflicts()
      setInterval(refreshConflicts, 10_000)
    } catch (e) {
      error = String(e)
    }
  })
</script>

<div class="app">
<div class="shell">
  <nav>
    <h1>Crate Digger</h1>
    {#each views as v}
      <button class:active={nav.view === v.id} onclick={() => { if (v.id === 'review') nav.reviewPlaylist = null; nav.view = v.id }}>
        {v.label}
        {#if v.id === 'identity' && conflicts > 0}<span class="badge">{conflicts}</span>{/if}
      </button>
    {/each}
    <footer>
      {#if info}v{info.version} · schema {info.schema_version}{/if}
    </footer>
  </nav>
  <main>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    {#if nav.view === 'review'}
      <Review />
    {:else if nav.view === 'activity'}
      <Activity />
    {:else if nav.view === 'library'}
      <Library />
    {:else if nav.view === 'map'}
      <LibraryMap />
    {:else if nav.view === 'playlists'}
      <Playlists />
    {:else if nav.view === 'identity'}
      <Identity onchange={refreshConflicts} />
    {:else if nav.view === 'settings'}
      <Settings />
    {:else}
      <p class="muted">{views.find((v) => v.id === nav.view)?.label} view</p>
    {/if}
  </main>
</div>
<PlayerBar />
</div>
{#if firstRun}
  <Onboarding settings={firstRun} ondone={() => ((firstRun = null), (nav.view = 'library'))} />
{/if}
