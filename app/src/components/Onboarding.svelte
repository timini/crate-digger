<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog'
  import { api, type AppSettings } from '../lib/api'
  import Connections from './Connections.svelte'
  import Folders from './Folders.svelte'

  let { settings, ondone }: { settings: AppSettings; ondone: () => void } = $props()

  const steps = ['Welcome', 'Music folders', 'Archive', 'Connections'] as const
  let step = $state(0)
  // The starting value only; later changes come from this dialog.
  // svelte-ignore state_referenced_locally
  let archive = $state(settings.archive_dir)
  let error: string | null = $state(null)

  async function chooseArchive() {
    const dir = await open({ directory: true, multiple: false, title: 'Where should kept tracks go?' })
    if (typeof dir !== 'string') return
    try {
      await api.setArchiveDir(dir)
      archive = dir
      error = null
    } catch (e) {
      error = String(e)
    }
  }

  async function finish() {
    await api.completeOnboarding()
    ondone()
  }
</script>

<div class="backdrop">
  <div class="dialog" role="dialog" aria-label="Set up Crate Digger">
    <ol class="steps">
      {#each steps as s, i}
        <li class:current={i === step} class:done={i < step}>{s}</li>
      {/each}
    </ol>

    {#if step === 0}
      <h2>Welcome to Crate Digger</h2>
      <p>A few quick steps. Every one can be skipped and changed later in Settings.</p>
    {:else if step === 1}
      <h2>Your music</h2>
      <p>Add the folders you keep music in.</p>
      <Folders />
    {:else if step === 2}
      <h2>Archive for kept tracks</h2>
      <p>When you keep a downloaded track, it moves here: <code>{archive}</code></p>
      <button onclick={chooseArchive}>Choose another folder...</button>
      {#if error}<p class="error">{error}</p>{/if}
    {:else}
      <h2>Connections</h2>
      <Connections />
    {/if}

    <footer>
      <button class="link" onclick={finish}>Skip setup</button>
      <span class="spacer"></span>
      {#if step > 0}<button onclick={() => step--}>Back</button>{/if}
      {#if step < steps.length - 1}
        <button class="primary" onclick={() => step++}>{step === 0 ? 'Start' : 'Next'}</button>
      {:else}
        <button class="primary" onclick={finish}>Done</button>
      {/if}
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.6);
    display: grid;
    place-items: center;
    z-index: 20;
  }
  .dialog {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 20px 28px;
    width: min(740px, 92vw);
    max-height: 90vh;
    overflow-y: auto;
  }
  .steps {
    display: flex;
    gap: 16px;
    list-style: none;
    padding: 0;
    color: var(--muted);
    font-size: 13px;
  }
  .steps .current {
    color: var(--accent);
  }
  .steps .done {
    color: var(--text);
  }
  footer {
    display: flex;
    gap: 8px;
    margin-top: 24px;
    align-items: center;
  }
  .spacer {
    flex: 1;
  }
  .link {
    background: none;
    border: none;
    color: var(--muted);
  }
  code {
    word-break: break-all;
  }
</style>
