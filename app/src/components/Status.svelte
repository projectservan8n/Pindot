<script lang="ts">
  import type { Status } from '../lib/status';

  let { status, onFix }: { status: Status; onFix?: () => void } = $props();
</script>

<div class="status" class:blocked={status.blocked} data-kind={status.kind} role="status" aria-live="polite" title={status.detail}>
  <span class="sdot" aria-hidden="true"></span>
  <span class="slabel">{status.label}</span>
  {#if status.blocked && onFix}<button type="button" class="link" onclick={onFix}>Fix in Setup</button>{/if}
  <span class="sr">{status.detail}</span>
</div>
