<!--
  Desktop-narrow filter popover. At wide widths (≥1281px) the review
  header renders the six filter chips inline; at narrower desktop
  widths the row becomes crowded enough that the chips wrap onto two
  or three sub-rows. This component is the inline alternative — a
  single `⚑ Filter [N]` trigger button with the chips tucked into a
  dropdown popover that opens on click.

  The popover-positioning + portal pattern mirrors ActionsMenu so the
  dropdown escapes the sticky header's transform-promoted stacking
  context (otherwise mobile Safari paints the menu behind the diff
  scrolling underneath).
-->
<script lang="ts">
  import { portal } from '../lib/portal';
  import type { ReviewToolbarState } from './ReviewViewer.svelte';

  type Filter = NonNullable<ReviewToolbarState['filter']>;
  type StatusKey = keyof Filter['status'];
  type FlagKey = keyof Filter['flag'];

  let {
    filter,
    activeCount,
  }: { filter: Filter; activeCount: number } = $props();

  let open = $state(false);
  let rootEl: HTMLDivElement | undefined = $state();
  let triggerEl: HTMLButtonElement | undefined = $state();
  let menuEl: HTMLDivElement | undefined = $state();
  let anchorTop = $state(0);
  let anchorLeft = $state(0);

  function recompute() {
    if (!triggerEl) return;
    const trig = triggerEl.getBoundingClientRect();
    anchorTop = trig.bottom + 4;
    // Right-align the popover with the trigger; clamp inside an 8px
    // viewport margin so a trigger near the right edge doesn't push
    // the panel off-screen.
    const menuWidth = menuEl?.offsetWidth ?? 320;
    const preferredLeft = trig.right - menuWidth;
    const minLeft = 8;
    const maxLeft = window.innerWidth - menuWidth - 8;
    anchorLeft = Math.max(minLeft, Math.min(maxLeft, preferredLeft));
  }

  function close() {
    open = false;
  }

  function toggle() {
    if (open) {
      close();
      return;
    }
    recompute();
    open = true;
  }

  $effect(() => {
    if (open && menuEl) recompute();
  });

  function onWindowClick(e: MouseEvent) {
    if (!open) return;
    const t = e.target as Node;
    if (rootEl && rootEl.contains(t)) return;
    if (menuEl && menuEl.contains(t)) return;
    close();
  }

  function onKey(e: KeyboardEvent) {
    if (open && e.key === 'Escape') {
      close();
      e.stopPropagation();
    }
  }

  function onScrollOrResize() {
    if (open) recompute();
  }

  // Portal so the popover escapes the sticky header's stacking
  // context (same reason ActionsMenu uses one).
</script>

<svelte:window
  onclick={onWindowClick}
  onkeydown={onKey}
  onscroll={onScrollOrResize}
  onresize={onScrollOrResize}
/>

<div class="filter-menu" bind:this={rootEl}>
  <button
    type="button"
    class="m-filter-btn"
    class:active={activeCount > 0}
    aria-haspopup="dialog"
    aria-expanded={open}
    aria-label="Filter comments"
    bind:this={triggerEl}
    onclick={(e) => {
      e.stopPropagation();
      toggle();
    }}
  >
    <span class="m-filter-glyph" aria-hidden="true">⚑</span>
    Filter
    {#if activeCount > 0}
      <span class="m-filter-badge">{activeCount}</span>
    {/if}
  </button>

  {#if open}
    <div
      class="menu"
      role="dialog"
      aria-label="Filter comments"
      bind:this={menuEl}
      use:portal
      style:top="{anchorTop}px"
      style:left="{anchorLeft}px"
    >
      <div class="filter-chips popover">
        <span class="label">Status</span>
        <button
          type="button"
          class="chip status-draft"
          class:on={filter.status.draft}
          aria-pressed={filter.status.draft}
          onclick={() => filter.toggleStatus('draft' as StatusKey)}
        >Draft</button>
        <button
          type="button"
          class="chip status-open"
          class:on={filter.status.open}
          aria-pressed={filter.status.open}
          onclick={() => filter.toggleStatus('open' as StatusKey)}
        >Open</button>
        <button
          type="button"
          class="chip status-resolved"
          class:on={filter.status.resolved}
          aria-pressed={filter.status.resolved}
          onclick={() => filter.toggleStatus('resolved' as StatusKey)}
        >Resolved</button>
        <span class="sep" aria-hidden="true"></span>
        <span class="label">Severity</span>
        <button
          type="button"
          class="chip flag-must-do"
          class:on={filter.flag['must-do']}
          aria-pressed={filter.flag['must-do']}
          onclick={() => filter.toggleFlag('must-do' as FlagKey)}
        >Must do</button>
        <button
          type="button"
          class="chip flag-suggestion"
          class:on={filter.flag.suggestion}
          aria-pressed={filter.flag.suggestion}
          onclick={() => filter.toggleFlag('suggestion' as FlagKey)}
        >Suggestion</button>
        <button
          type="button"
          class="chip flag-question"
          class:on={filter.flag.question}
          aria-pressed={filter.flag.question}
          onclick={() => filter.toggleFlag('question' as FlagKey)}
        >Question</button>
      </div>
      {#if filter.hiddenCount > 0}
        <button
          type="button"
          class="sheet-reset"
          onclick={() => { filter.reset(); close(); }}
        >
          Show all — {filter.hiddenCount} hidden
        </button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .filter-menu {
    position: relative;
    display: inline-block;
  }

  /* `position: fixed` + portal so the dropdown escapes the sticky
   * header's transform-promoted stacking context. Mirrors ActionsMenu's
   * approach — see its style block for the full rationale. */
  .menu {
    position: fixed;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15);
    z-index: 1000;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    /* Two-line layout target: status row + severity row laid out by
     * the existing .filter-chips wrap, with enough horizontal room
     * that all six chips fit at two rows max. */
    max-width: min(420px, calc(100vw - 16px));
  }

  /* Inside the popover, let the chip cluster wrap onto two rows
   * naturally — the popover is wide enough for status on one row and
   * severity on the next. */
  :global(.filter-chips.popover) {
    flex-wrap: wrap;
  }

  .sheet-reset {
    align-self: flex-start;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--link);
    padding: 4px 10px;
    border-radius: 4px;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .sheet-reset:hover {
    background: var(--bg-elevated);
  }
</style>
