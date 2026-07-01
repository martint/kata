<!--
  Lightweight dropdown menu attached to a kebab (or arbitrary) trigger.
  Used for per-row actions on the review list and the analogous menu in
  the review viewer header. The menu items are data-driven via the
  `items` prop so callers don't have to repeat the open/close plumbing.

  Trigger button stops click propagation so a menu opened from inside a
  clickable row doesn't double-fire the row's own action. Each menu
  item closes the menu before firing its handler.
-->
<script lang="ts">
  import { portal } from '../lib/portal';

  type Item = {
    label: string;
    onclick: () => void;
    danger?: boolean;
    disabled?: boolean;
  };

  let {
    items,
    label = 'Actions',
    trigger = '⋯',
  }: { items: Item[]; label?: string; trigger?: string } = $props();

  let open = $state(false);
  let rootEl: HTMLDivElement | undefined = $state();
  let triggerEl: HTMLButtonElement | undefined = $state();
  let menuEl: HTMLDivElement | undefined = $state();
  /** Viewport-coord anchor for the menu, captured on open. The menu
   *  itself renders with `position: fixed` (anchored to these coords)
   *  so it escapes the sticky header's `transform`-promoted stacking
   *  context — otherwise mobile Safari paints the dropdown behind the
   *  body content that scrolls beneath the header. */
  let anchorTop = $state(0);
  let anchorLeft = $state(0);

  function recompute() {
    if (!triggerEl) return;
    const trig = triggerEl.getBoundingClientRect();
    anchorTop = trig.bottom + 4;
    // Default to right-aligning the menu with the trigger so the
    // dropdown reads as anchored to the kebab; clamp inside an 8px
    // margin if that would push the menu off either viewport edge
    // (a kebab near the left edge of a narrow phone row would
    // otherwise crop the menu's left half off-screen).
    const menuWidth = menuEl?.offsetWidth ?? 160;
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

  /** Re-measure once the dropdown has mounted: the first `recompute`
   *  inside `toggle` runs before `menuEl` exists so it has to estimate
   *  the menu width. After mount we can read the real `offsetWidth`
   *  and adjust if the estimate was off (matters when content is
   *  wider than the 160px fallback). */
  $effect(() => {
    if (open && menuEl) recompute();
  });

  function onWindowClick(e: MouseEvent) {
    if (!open) return;
    const t = e.target as Node;
    if (rootEl && rootEl.contains(t)) return;
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

  function run(item: Item) {
    close();
    item.onclick();
  }

  // Portal the dropdown to `document.body` on mount so it escapes
  // every `transform` / `filter` / `contain` containing block on the
  // way down from the document root — without this, `position:
  // fixed` on the menu resolves against the nearest transformed
  // ancestor (e.g. `.review-list .row-actions`'s `translateY(-50%)`)
  // and the dropdown ends up off-screen. See `../lib/portal.ts`.
</script>

<svelte:window
  onclick={onWindowClick}
  onkeydown={onKey}
  onscroll={onScrollOrResize}
  onresize={onScrollOrResize}
/>

<div class="actions-menu" bind:this={rootEl}>
  <button
    type="button"
    class="trigger"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={label}
    bind:this={triggerEl}
    onclick={(e) => {
      e.stopPropagation();
      toggle();
    }}
  >
    {trigger}
  </button>
  {#if open}
    <div
      class="menu"
      role="menu"
      bind:this={menuEl}
      use:portal
      style:top="{anchorTop}px"
      style:left="{anchorLeft}px"
    >
      {#each items as item (item.label)}
        <button
          type="button"
          role="menuitem"
          class:danger={item.danger}
          disabled={item.disabled}
          onclick={(e) => {
            e.stopPropagation();
            run(item);
          }}
        >
          {item.label}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .actions-menu {
    position: relative;
    display: inline-block;
  }

  .trigger {
    background: transparent;
    border: 1px solid transparent;
    color: inherit;
    padding: 2px 8px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 16px;
    line-height: 1;
    /* Tap target on phones — the kebab itself is small but the hit
     * area should reach the comfortable ≥32px minimum. */
    min-width: 32px;
    min-height: 28px;
  }

  .trigger:hover,
  .trigger[aria-expanded='true'] {
    background: var(--bg-elevated);
    border-color: var(--border);
  }

  /* `position: fixed` (not `absolute`) so the dropdown escapes the
   * sticky header's transform-promoted stacking context. The header
   * uses `transform: translateZ(0)` + `isolation: isolate` to prevent
   * diff text bleeding through on iOS Safari, but those promotions
   * also clip any in-context absolutely-positioned descendant that
   * extends past the header's box — the dropdown would render below
   * the page content scrolling underneath. Anchoring to viewport
   * coords (computed from the trigger's rect on open) sidesteps the
   * issue entirely. */
  .menu {
    position: fixed;
    min-width: 140px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15);
    z-index: 1000;
    display: flex;
    flex-direction: column;
    padding: 4px;
  }

  .menu button {
    text-align: left;
    background: transparent;
    border: none;
    padding: 6px 10px;
    border-radius: 4px;
    cursor: pointer;
    font: inherit;
    color: inherit;
    white-space: nowrap;
  }

  .menu button:hover:not(:disabled) {
    background: var(--bg-elevated);
  }

  .menu button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .menu button.danger {
    color: var(--error-text);
  }
</style>
