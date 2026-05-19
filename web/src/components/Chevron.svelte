<script lang="ts">
  //! Small SVG chevron used by every prev/next-style nav button. Drawn
  //! as a centered stroke so it lines up inside the button regardless
  //! of the surrounding font's metrics — the Unicode ‹ › glyphs sit
  //! off-baseline in most fonts and look misaligned in square buttons.
  //!
  //! `filled` switches the geometry from an open chevron to a solid
  //! disclosure triangle — used by the thread-fold marker, where the
  //! tiny stroked chevron disappeared against the surrounding diff
  //! and didn't read as a clickable affordance on a clean row.

  interface Props {
    dir: 'left' | 'right' | 'up' | 'down';
    size?: number;
    filled?: boolean;
  }
  const { dir, size = 14, filled = false }: Props = $props();

  const rotation = $derived(
    dir === 'left' ? 180 : dir === 'right' ? 0 : dir === 'up' ? -90 : 90,
  );
</script>

<svg
  class="chevron"
  width={size}
  height={size}
  viewBox="0 0 16 16"
  aria-hidden="true"
  focusable="false"
  style:transform="rotate({rotation}deg)"
>
  {#if filled}
    <polygon points="5,3 12,8 5,13" fill="currentColor" />
  {:else}
    <path
      d="M6 4 L10 8 L6 12"
      stroke="currentColor"
      stroke-width="1.6"
      fill="none"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
  {/if}
</svg>

<style>
  .chevron {
    display: inline-block;
    vertical-align: middle;
  }
</style>
