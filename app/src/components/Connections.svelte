<script lang="ts">
  import { onMount } from 'svelte'
  import { connections, type Connections, type Seed } from '../lib/api'
  import Soulseek from './Soulseek.svelte'
  let config: Connections | null = $state(null)
  let seeds: Seed[] = $state([])
  let message = $state('')
  let error = $state('')
  let busy = $state(false)
  let secrets: Record<string, string> = $state({})
  const credentials = [
    ['llm', 'Model API key (not needed for local models)'],
    ['discogs', 'Discogs personal token'], ['youtube', 'YouTube Data API key'],
    ['soulseek_username', 'Soulseek username'], ['soulseek_password', 'Soulseek password'],
    ['slskd', 'External slskd API key'],
    ['acoustid', 'AcoustID application key (for identifying library tracks)'],
    ['google_client_secret', 'Google client secret (only if your client id needs one)'],
  ]
  async function run(action: () => Promise<unknown>, success = 'Saved') {
    busy = true; error = ''; message = ''
    try { const result = await action(); message = typeof result === 'string' ? result : success }
    catch (e) { error = String(e) }
    finally { busy = false }
  }
  onMount(() => { void run(async () => {
    config = await connections.get(); seeds = await connections.seeds()
  }, '') })
  const defaultEndpoints: Record<string, string> = {
    openai_compatible: 'http://localhost:11434/v1',
    anthropic: 'https://api.anthropic.com/v1',
  }
  function providerChanged() {
    if (config && Object.values(defaultEndpoints).includes(config.llm_endpoint)) {
      config.llm_endpoint = defaultEndpoints[config.llm_provider]
    }
  }
  async function saveCredential(key: string) {
    const value = secrets[key]
    if (!value) return
    await connections.credential(key, value)
    secrets[key] = ''
  }
</script>

<p class="muted">Every connection is optional. Secrets are stored in your operating system credential store.</p>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p role="status">{message}</p>{/if}
{#if config}
  <fieldset disabled={busy}>
    <legend>Model and Soulseek</legend>
    <label>Model provider <select bind:value={config.llm_provider} onchange={providerChanged}>
      <option value="openai_compatible">OpenAI compatible (including Ollama, LM Studio and llama.cpp)</option>
      <option value="anthropic">Anthropic</option>
    </select></label>
    <label>Model endpoint <input bind:value={config.llm_endpoint} placeholder="http://localhost:11434/v1" /></label>
    <label>Model name <input bind:value={config.llm_model} placeholder="qwen2.5-coder:latest" /></label>
    <label class="check"><input type="checkbox" bind:checked={config.enabled} />Discover from seeds and ratings every 6 hours while the app is open</label>
    <label class="check"><input type="checkbox" bind:checked={config.external_slskd} />Use an existing slskd instance</label>
    {#if config.external_slskd}
      <label>slskd endpoint <input bind:value={config.slskd_endpoint} /></label>
      <label>slskd downloads folder <input bind:value={config.slskd_downloads_dir} placeholder="The folder your slskd saves finished downloads in" /></label>
    {/if}
    <label>Shared catalogue address <input bind:value={config.central_endpoint} placeholder="https://catalogue.example.run.app" /></label>
    <label>Google client id <input bind:value={config.google_client_id} placeholder="1234-abc.apps.googleusercontent.com" /></label>
    <button onclick={() => run(() => connections.save(config!))}>Save connections</button>
    <Soulseek />
  </fieldset>
  <fieldset disabled={busy}>
    <legend>Credentials</legend>
    {#each credentials as [key, label]}
      <div class="credential">
        <label>{label}<input type="password" autocomplete="off" bind:value={secrets[key]} /></label>
        <button disabled={!secrets[key]} onclick={() => run(() => saveCredential(key))}>Save</button>
        <button onclick={() => run(() => connections.credential(key, null), 'Credential removed')}>Remove</button>
      </div>
    {/each}
    <div class="buttons">
      {#each ['llm', 'discogs', 'youtube', 'slskd'] as service}
        <button onclick={() => run(() => connections.test(service))}>Test {service}</button>
      {/each}
    </div>
  </fieldset>
  <fieldset disabled={busy}>
    <legend>Discovery seeds</legend>
    {#each seeds as seed, i}
      <div class="seed">
        <select aria-label="Seed kind" bind:value={seed.kind}>
          {#each ['artist', 'label', 'dj', 'track'] as kind}<option value={kind}>{kind}</option>{/each}
        </select>
        <input aria-label="Seed value" bind:value={seed.value} />
        <button aria-label="Remove seed" onclick={() => seeds = seeds.filter((_, index) => index !== i)}>Remove</button>
      </div>
    {/each}
    <button onclick={() => seeds = [...seeds, { kind: 'artist', value: '' }]}>Add seed</button>
    <button onclick={() => run(() => connections.saveSeeds(seeds))}>Save seeds</button>
  </fieldset>
{/if}
<style>
  fieldset { border: 1px solid var(--border); margin: 12px 0; display: grid; gap: 10px; }
  label { display: grid; gap: 4px; }
  .check, .buttons, .seed, .credential { display: flex; align-items: center; gap: 8px; }
  .credential label { flex: 1; }
  input, select { min-width: 0; }
  .seed input { flex: 1; }
  .buttons { flex-wrap: wrap; }
</style>
