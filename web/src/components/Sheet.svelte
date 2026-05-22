<!--
  Bottom sheet — a mobile-only surface that slides up from the
  bottom edge over a dimmed backdrop. Used by the mobile review
  header to hold the filter chips and the overflow ("More")
  controls, which there isn't room to keep permanently on screen.

  Closes on backdrop tap, the ✕ button, or Escape. The caller owns
  the open/closed state and simply stops rendering `<Sheet>` to
  close it; the slide-out transition still plays on unmount.
-->
<script lang="ts">
  import { fade, fly } from 'svelte/transition';
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    onclose: () => void;
    children: Snippet;
  }
  const { title, onclose, children }: Props = $props();

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div
  class="sheet-backdrop"
  role="presentation"
  onclick={onclose}
  transition:fade={{ duration: 150 }}
></div>
<div
  class="sheet"
  role="dialog"
  aria-modal="true"
  aria-label={title}
  transition:fly={{ y: 320, duration: 200 }}
>
  <header class="sheet-head">
    <span class="sheet-title">{title}</span>
    <button
      type="button"
      class="sheet-close"
      onclick={onclose}
      aria-label="Close"
    >✕</button>
  </header>
  <div class="sheet-body">
    {@render children()}
  </div>
</div>

<style>
  .sheet-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 60;
  }

  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 61;
    display: flex;
    flex-direction: column;
    max-height: 80vh;
    background: var(--bg);
    border-top: 1px solid var(--border);
    border-radius: 14px 14px 0 0;
    box-shadow: 0 -8px 28px rgba(0, 0, 0, 0.28);
  }

  .sheet-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-muted);
  }

  .sheet-title {
    font-size: 14px;
    font-weight: 600;
  }

  .sheet-close {
    width: 32px;
    height: 32px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    font-size: 16px;
    cursor: pointer;
  }
  .sheet-close:hover {
    background: var(--bg-elevated);
  }

  .sheet-body {
    padding: 16px;
    overflow-y: auto;
  }
</style>
