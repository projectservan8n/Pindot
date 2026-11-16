<script lang="ts">
  // A small picture of an attached image, fetched from Pindot's own attachments
  // folder through the app (the page never reads files itself).
  import { api } from '../lib/api';

  let { path, name, big = false }: { path: string; name: string; big?: boolean } = $props();
  let src = $state<string | null>(null);
  let failed = $state(false);
  let zoom = $state(false);

  $effect(() => {
    const p = path;
    src = null;
    failed = false;
    api.chatImage(p).then(
      (s) => {
        if (p === path) src = s;
      },
      () => (failed = true),
    );
  });
</script>

{#if src}
  <button class="thumb" class:big type="button" onclick={() => (zoom = true)} aria-label={`View picture ${name}`}>
    <img {src} alt={name} />
  </button>
  {#if zoom}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="lightbox" role="dialog" aria-modal="true" aria-label={name} tabindex="-1" onclick={() => (zoom = false)} onkeydown={(e) => e.key === 'Escape' && (zoom = false)}>
      <img {src} alt={name} />
      <button class="primary" type="button" onclick={() => (zoom = false)}>Close</button>
    </div>
  {/if}
{:else}
  <span class="thumb ph" class:big title={failed ? 'This picture is no longer on this computer' : 'Loading'}>{failed ? '?' : ''}</span>
{/if}
