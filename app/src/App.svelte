<script lang="ts">
  import { onMount } from 'svelte'
  import { api, type AppInfo } from './lib/api'

  type View = 'review' | 'library' | 'playlists' | 'activity' | 'settings'
  const views: { id: View; label: string }[] = [
    { id: 'review', label: 'Review' },
    { id: 'library', label: 'Library' },
    { id: 'playlists', label: 'Playlists' },
    { id: 'activity', label: 'Activity' },
    { id: 'settings', label: 'Settings' },
  ]

  let current: View = $state('library')
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
    <p class="muted">{views.find((v) => v.id === current)?.label} view</p>
  </main>
</div>
