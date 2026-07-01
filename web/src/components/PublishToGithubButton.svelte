<!--
  Split button that publishes the current draft session as a GitHub
  PR review. The primary click is the safe default — submit with
  `event=COMMENT` (no approval, no block). The dropdown caret opens
  a small menu offering the two stronger events:

    - Approve: submit with `event=APPROVE`. Optional body.
    - Request changes: submit with `event=REQUEST_CHANGES`. Body is
      required by GitHub for this event.

  Both non-neutral events open a shared `PublishBodyModal` for body
  capture — Request changes forces a non-empty body per GitHub's
  own rule; Approve keeps it optional.

  The popover-positioning + portal pattern mirrors FilterMenu so the
  dropdown escapes the sticky toolbar's stacking context.
-->
<script lang="ts">
  import { portal } from '../lib/portal';
  import PublishBodyModal from './PublishBodyModal.svelte';

  type PublishEvent = 'COMMENT' | 'APPROVE' | 'REQUEST_CHANGES';

  let {
    publish,
    saving,
  }: {
    /** Returns `true` on success, `false` on any failure the caller
     *  already surfaced (e.g. the page-level error banner or the
     *  head-drift confirm's cancel path). The modal uses that
     *  signal to stay open on failure so the user's typed body
     *  isn't lost. */
    publish: (event: PublishEvent, body?: string) => Promise<boolean>;
    saving: boolean;
  } = $props();

  let menuOpen = $state(false);
  let rootEl: HTMLDivElement | undefined = $state();
  let caretEl: HTMLButtonElement | undefined = $state();
  let menuEl: HTMLDivElement | undefined = $state();
  let anchorTop = $state(0);
  let anchorLeft = $state(0);

  /** `null` when closed. The event determines body optionality. */
  let modalEvent = $state<null | 'APPROVE' | 'REQUEST_CHANGES'>(null);

  function recompute() {
    if (!caretEl) return;
    const trig = caretEl.getBoundingClientRect();
    anchorTop = trig.bottom + 4;
    // Fallback must match the CSS `min-width` below (280) so the
    // first-open position doesn't clamp against a smaller width and
    // then jump on the second frame once `menuEl.offsetWidth` reads.
    const menuWidth = menuEl?.offsetWidth ?? 280;
    const preferredLeft = trig.right - menuWidth;
    const minLeft = 8;
    const maxLeft = window.innerWidth - menuWidth - 8;
    anchorLeft = Math.max(minLeft, Math.min(maxLeft, preferredLeft));
  }

  function closeMenu() {
    menuOpen = false;
    // Return focus to the caret on close so a keyboard/AT user
    // isn't stranded — the menu was portalled out of the button's
    // DOM subtree, so browser focus doesn't naturally return.
    caretEl?.focus();
  }

  function toggleMenu() {
    if (menuOpen) {
      closeMenu();
      return;
    }
    recompute();
    menuOpen = true;
  }

  $effect(() => {
    if (menuOpen && menuEl) {
      recompute();
      // Move focus into the menu on open (first item) so keyboard/AT
      // users can operate the dropdown. Without this the menu is
      // announced but unreachable — `role="menu"` promises a
      // keyboard model that isn't there otherwise.
      focusMenuItem(0);
    }
  });

  function menuItems(): HTMLButtonElement[] {
    if (!menuEl) return [];
    return Array.from(menuEl.querySelectorAll<HTMLButtonElement>('button[role="menuitem"]'));
  }

  function focusMenuItem(index: number) {
    const items = menuItems();
    if (items.length === 0) return;
    const clamped = ((index % items.length) + items.length) % items.length;
    items[clamped]?.focus();
  }

  function onMenuKey(e: KeyboardEvent) {
    if (!menuOpen) return;
    const items = menuItems();
    if (items.length === 0) return;
    const active = document.activeElement as HTMLElement | null;
    const currentIdx = items.findIndex((el) => el === active);
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        focusMenuItem(currentIdx < 0 ? 0 : currentIdx + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        focusMenuItem(currentIdx < 0 ? items.length - 1 : currentIdx - 1);
        break;
      case 'Home':
        e.preventDefault();
        focusMenuItem(0);
        break;
      case 'End':
        e.preventDefault();
        focusMenuItem(items.length - 1);
        break;
      case 'Tab':
        // Tab out of the menu closes it — matches the WAI-ARIA
        // menu pattern (Tab moves to the next tab stop, which
        // implies the menu is dismissed).
        closeMenu();
        break;
    }
  }

  function onWindowClick(e: MouseEvent) {
    if (!menuOpen) return;
    const t = e.target as Node;
    if (rootEl && rootEl.contains(t)) return;
    if (menuEl && menuEl.contains(t)) return;
    closeMenu();
  }

  function onKey(e: KeyboardEvent) {
    if (menuOpen && e.key === 'Escape') {
      closeMenu();
      e.stopPropagation();
      return;
    }
    onMenuKey(e);
  }

  function onScrollOrResize() {
    if (menuOpen) recompute();
  }


  function openModal(event: 'APPROVE' | 'REQUEST_CHANGES') {
    closeMenu();
    modalEvent = event;
  }

  async function submitModal(body: string | undefined) {
    // Keep the modal mounted through the await so a failed publish
    // (network, head-drift decline, backend refusal) doesn't discard
    // the user's typed body — for REQUEST_CHANGES that body is
    // required and can be several paragraphs. Only close on success.
    const event = modalEvent;
    if (!event) return;
    const ok = await publish(event, body);
    if (ok) modalEvent = null;
  }

  async function submitComment() {
    closeMenu();
    await publish('COMMENT');
  }
</script>

<svelte:window
  onclick={onWindowClick}
  onkeydown={onKey}
  onscroll={onScrollOrResize}
  onresize={onScrollOrResize}
/>

<div class="split-button" bind:this={rootEl}>
  <button
    type="button"
    class="primary main"
    onclick={submitComment}
    disabled={saving}
    title="Publish drafts as a GitHub PR review (event=COMMENT)"
    data-tour="publish-to-github"
  >
    {saving ? 'Publishing…' : 'Publish to GitHub'}
  </button>
  <button
    type="button"
    class="primary caret"
    aria-haspopup="menu"
    aria-expanded={menuOpen}
    aria-label="More publish options"
    bind:this={caretEl}
    disabled={saving}
    onclick={(e) => {
      e.stopPropagation();
      toggleMenu();
    }}
  >
    <span aria-hidden="true">▾</span>
  </button>

  {#if menuOpen}
    <div
      class="menu"
      role="menu"
      aria-label="Publish options"
      bind:this={menuEl}
      use:portal
      style:top="{anchorTop}px"
      style:left="{anchorLeft}px"
    >
      <button
        type="button"
        role="menuitem"
        class="menu-item"
        onclick={() => openModal('APPROVE')}
      >
        <span class="ico approve" aria-hidden="true">✓</span>
        <span class="item-text">
          <span class="item-title">Publish &amp; approve</span>
          <span class="item-sub">Submit as event=APPROVE.</span>
        </span>
      </button>
      <button
        type="button"
        role="menuitem"
        class="menu-item"
        onclick={() => openModal('REQUEST_CHANGES')}
      >
        <span class="ico request" aria-hidden="true">✗</span>
        <span class="item-text">
          <span class="item-title">Publish &amp; request changes</span>
          <span class="item-sub">Submit as event=REQUEST_CHANGES. Body required.</span>
        </span>
      </button>
    </div>
  {/if}
</div>

{#if modalEvent}
  <PublishBodyModal
    event={modalEvent}
    {saving}
    returnFocusTo={caretEl ?? null}
    onsubmit={submitModal}
    onclose={() => (modalEvent = null)}
  />
{/if}

<style>
  .split-button {
    display: inline-flex;
    align-items: stretch;
  }

  .split-button .primary.main {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
    margin-right: 0;
  }

  .split-button .primary.caret {
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    border-left: 1px solid rgba(0, 0, 0, 0.18);
    padding-left: 8px;
    padding-right: 8px;
    min-width: 28px;
  }

  .menu {
    position: fixed;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18);
    z-index: 1000;
    padding: 4px;
    display: flex;
    flex-direction: column;
    min-width: 280px;
  }

  .menu-item {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    background: transparent;
    border: none;
    text-align: left;
    padding: 8px 10px;
    border-radius: 4px;
    cursor: pointer;
    color: inherit;
    font: inherit;
  }
  .menu-item:hover {
    background: var(--bg-elevated);
  }

  .ico {
    flex: 0 0 18px;
    height: 18px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    color: white;
    font-size: 11px;
    font-weight: 700;
    margin-top: 2px;
  }
  .ico.approve { background: #2da44e; }
  .ico.request { background: #cf222e; }

  .item-text {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .item-title {
    font-size: 13px;
    font-weight: 600;
  }
  .item-sub {
    font-size: 11px;
    color: var(--text-muted);
  }
</style>
