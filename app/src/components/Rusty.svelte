<script lang="ts">
  // The Rusty mark (docs/brand) with eyes that can move, so state shows on
  // the mascot itself. Poses: idle, listening, thinking, acting, speaking,
  // off, blocked. Motion is CSS and stops under reduced motion.
  let { size = 28, pose = 'idle' }: { size?: number; pose?: string } = $props();
</script>

<svg class="rusty" data-pose={pose} viewBox="40 40 176 176" width={size} height={size} aria-hidden="true">
  <path
    fill="var(--brand)"
    fill-rule="evenodd"
    d="M46.12 71.59 A20 20 0 0 1 71.59 46.12 L197.59 90.12 A20 20 0 0 1 200.99 126.32 L153.64 153.64 L126.32 200.99 A20 20 0 0 1 90.12 197.59 Z"
  />
  <rect class="eye" x="97" y="105" width="16" height="28" rx="8" />
  <rect class="eye" x="127" y="105" width="16" height="28" rx="8" />
</svg>
