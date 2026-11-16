<script lang="ts">
  import type { Status } from '../lib/status';
  import Rusty from './Rusty.svelte';

  let { status, onFix }: { status: Status; onFix?: () => void } = $props();
  const pose = $derived(
    status.blocked ? 'blocked' : status.kind === 'ready' || status.kind === 'starting' ? 'idle' : status.kind
  );
</script>

<div class="status" class:blocked={status.blocked} data-kind={status.kind} role="status" aria-live="polite" title={status.detail}>
  <span class="rusty-sm"><Rusty size={20} {pose} /></span>
  <span class="slabel">{status.label}</span>
  {#if status.blocked && onFix}<button type="button" class="link" onclick={onFix}>Fix in Setup</button>{/if}
  <span class="sr">{status.detail}</span>
</div>
