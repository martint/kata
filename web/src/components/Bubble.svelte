<script lang="ts">
  //! Speech-bubble glyph for "add a comment" affordances. Drawn as a
  //! rounded rectangle with a small triangular tail; `currentColor`
  //! lets it pick up the surrounding text color so the button styles
  //! its own state.

  interface Props {
    size?: number;
  }
  const { size = 14 }: Props = $props();
</script>

<svg
  class="bubble"
  width={size}
  height={size}
  viewBox="0 0 16 16"
  aria-hidden="true"
  focusable="false"
>
  <path
    d="M2.4 3 H13.6 A1.4 1.4 0 0 1 15 4.4 V10 A1.4 1.4 0 0 1 13.6 11.4 H6.6 L4 14 V11.4 H2.4 A1.4 1.4 0 0 1 1 10 V4.4 A1.4 1.4 0 0 1 2.4 3 Z"
    stroke="currentColor"
    stroke-width="1.4"
    fill="none"
    stroke-linejoin="round"
  />
</svg>

<style>
  .bubble {
    display: inline-block;
    vertical-align: middle;
  }
</style>
