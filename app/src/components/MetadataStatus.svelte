<script lang="ts">
  import { metadata, type MetadataStatus } from '../lib/api'

  let { trackId }: { trackId: string } = $props()
  let status: MetadataStatus | null = $state(null)

  const text: Record<MetadataStatus['status'], string> = {
    queued: 'Looking up metadata.',
    identified: 'Identified from the audio.',
    suggested: 'Possible matches found; check them in the Library.',
    not_found: 'No match found.',
    waiting: 'Waiting',
    failed: 'Lookup failed',
  }

  async function load() {
    status = (await metadata.status(trackId).catch(() => null)) ?? null
  }
  $effect(() => {
    void trackId
    void load()
  })
</script>

<p class="muted small">
  {#if status}
    {text[status.status]}{#if status.detail && status.status !== 'suggested'}{' '}{status.detail}{/if}
  {:else}
    Not looked up yet.
  {/if}
  <button class="link" onclick={async () => (await metadata.identify(trackId), await load())}>Look up again</button>
</p>
