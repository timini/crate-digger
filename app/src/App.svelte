<script lang="ts">
  import { onMount } from 'svelte'
  import { api, type AppInfo } from './lib/api'
  import Activity from './views/Activity.svelte'
  import Library from './views/Library.svelte'
  import PlayerBar from './components/PlayerBar.svelte'
  import Playlists from './views/Playlists.svelte'
  import Review from './views/Review.svelte'

  type View = 'review' | 'library' | 'playlists' | 'activity' | 'settings'
  const views: { id: View; label: string }[] = [
    { id: 'review', label: 'Review' },
    { id: 'library', label: 'Library' },
    { id: 'playlists', label: 'Playlists' },
    { id: 'activity', label: 'Activity' },
    { id: 'settings', label: 'Settings' },
  ]

  let current: View = $state('review')
  let info: AppInfo | null = $state(null)
  let error: string | null = $state(null)

  onMount(async () => {
    try {
      info = await api.appInfo()
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
      <button class:active={current === v.id} onclick={() => (current = v.id)}>{v.label}</button>
    {/each}
    <footer>
      {#if info}v{info.version} · schema {info.schema_version}{/if}
    </footer>
  </nav>
  <main>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    {#if current === 'review'}
      <Review />
    {:else if current === 'activity'}
      <Activity />
    {:else if current === 'library'}
      <Library />
    {:else if current === 'playlists'}
      <Playlists />
    {:else}
      <p class="muted">{views.find((v) => v.id === current)?.label} view</p>
    {/if}
  </main>
</div>
<PlayerBar />
</div>
