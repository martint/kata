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
  fill="currentColor"
>
  <path d="M1 2.75A2.75 2.75 0 0 1 3.75 0h8.5A2.75 2.75 0 0 1 15 2.75v6.5A2.75 2.75 0 0 1 12.25 12H8.06l-2.573 2.573A1.457 1.457 0 0 1 3 13.543V12h-.25A1.75 1.75 0 0 1 1 10.25v-7.5z"/>
</svg>

<style>
  .bubble {
    display: inline-block;
    vertical-align: middle;
  }
</style>
