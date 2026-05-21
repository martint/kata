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

  function close() {
    open = false;
  }

  function onWindowClick(e: MouseEvent) {
    if (!open) return;
    if (rootEl && !rootEl.contains(e.target as Node)) close();
  }

  function onKey(e: KeyboardEvent) {
    if (open && e.key === 'Escape') {
      close();
      e.stopPropagation();
    }
  }

  function run(item: Item) {
    close();
    item.onclick();
  }
</script>

<svelte:window onclick={onWindowClick} onkeydown={onKey} />

<div class="actions-menu" bind:this={rootEl}>
  <button
    type="button"
    class="trigger"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={label}
    onclick={(e) => {
      e.stopPropagation();
      open = !open;
    }}
  >
    {trigger}
  </button>
  {#if open}
    <div class="menu" role="menu">
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

  .menu {
    position: absolute;
    right: 0;
    top: 100%;
    min-width: 140px;
    margin-top: 4px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15);
    z-index: 50;
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
