<script lang="ts">
  //! Floating popup that appears next to a diff text selection,
  //! offering selection-scoped actions: open the comment composer,
  //! copy the selected text as plain text, or copy a permalink to
  //! the line range. Dumb-component shape — the caller (HunkLines /
  //! HunkLinesSideBySide) owns the `DiffSelection` state and the
  //! dismissal logic; the popup just renders the affordance and
  //! routes clicks to callbacks.
  //!
  //! Positioned in document coords (rect.bottom + scrollY) just below
  //! and right of the selection's end, so the popup doesn't sit on
  //! top of the highlighted text the reviewer just picked.

  import type { DiffSelection } from '../lib/diffSelection';
  import Bubble from './Bubble.svelte';

  interface Props {
    selection: DiffSelection | null;
    /** Viewport-coord anchor for the popup. Caller passes the
     *  mouseup pointer position so the popup lands where the user
     *  actually let go — `selection.rect` is the whole selection's
     *  bounding box, which for multi-line drags can be hundreds of
     *  pixels wide and put the popup off-screen. */
    anchorX: number;
    anchorY: number;
    /** Open the comment composer anchored to the selection's line
     *  range (and column range for multi-line). */
    oncomment: () => void;
    /** Copy the selected text to the clipboard as plain text. */
    oncopy: () => void;
    /** Copy a permalink URL pointing at the selection's line range
     *  to the clipboard. */
    onpermalink: () => void;
  }
  const { selection, anchorX, anchorY, oncomment, oncopy, onpermalink }: Props = $props();

  // Approximate popup footprint — three 26-px buttons + 2-px gaps +
  // 3-px padding on each side. Used to flip the popup above / left
  // of the anchor when it would otherwise overflow the viewport.
  const POPUP_W = 90;
  const POPUP_H = 32;
  const GAP = 6;

  // Viewport-coord positioning. The popup uses `position: fixed` so
  // its coord system is the viewport directly — `position: absolute`
  // would offset by the nearest positioned ancestor (`.hunks-wrapper`
  // here), which sits at an unpredictable document-y once the file
  // is scrolled, landing the popup off-screen for any file below
  // the first one.
  const top = $derived.by(() => {
    if (!selection || typeof window === 'undefined') return 0;
    // Below the pointer by default; flip above if we'd overflow the
    // viewport bottom.
    if (anchorY + GAP + POPUP_H <= window.innerHeight) return anchorY + GAP;
    return Math.max(0, anchorY - GAP - POPUP_H);
  });
  const left = $derived.by(() => {
    if (!selection || typeof window === 'undefined') return 0;
    // Right of the pointer by default; flip left if we'd overflow
    // the viewport right edge. Clamp to a small margin so the popup
    // is never glued to the very edge.
    if (anchorX + GAP + POPUP_W <= window.innerWidth) return anchorX + GAP;
    return Math.max(4, anchorX - GAP - POPUP_W);
  });
</script>

{#if selection}
  <div
    class="selection-popup"
    role="toolbar"
    aria-label="Selection actions"
    tabindex="-1"
    style:top="{top}px"
    style:left="{left}px"
  >
    <button
      type="button"
      class="popup-btn"
      title="Comment on selection"
      aria-label="Comment on selection"
      onmousedown={(e) => e.preventDefault()}
      onclick={oncomment}
    >
      <Bubble size={14} />
    </button>
    <button
      type="button"
      class="popup-btn"
      title="Copy selected text"
      aria-label="Copy selected text"
      onmousedown={(e) => e.preventDefault()}
      onclick={oncopy}
    >
      <!-- Stacked-rectangle "copy" glyph: the standard two-overlapping-
           pages metaphor. `currentColor` so the button styles its own
           state. -->
      <svg
        width="14"
        height="14"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
        focusable="false"
      >
        <rect x="5" y="5" width="9" height="10" rx="1.2" />
        <path d="M11 5 V3.2 A1.2 1.2 0 0 0 9.8 2 H3.2 A1.2 1.2 0 0 0 2 3.2 V9.8 A1.2 1.2 0 0 0 3.2 11 H5" />
      </svg>
    </button>
    <button
      type="button"
      class="popup-btn"
      title="Copy permalink"
      aria-label="Copy permalink"
      onmousedown={(e) => e.preventDefault()}
      onclick={onpermalink}
    >
      <!-- Two-link-segments "chain" glyph for permalink. -->
      <svg
        width="14"
        height="14"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
        focusable="false"
      >
        <path d="M6.5 10.5 L9.5 7.5" />
        <path d="M9.5 4 L10.5 3 a2.5 2.5 0 0 1 3.5 3.5 L13 7.5" />
        <path d="M6.5 14 L5.5 15 a2.5 2.5 0 0 1 -3.5 -3.5 L3 10.5" />
      </svg>
    </button>
  </div>
{/if}

<style>
  .selection-popup {
    /* `fixed` so the popup's coord system matches the viewport-
     * relative rect we get from `getBoundingClientRect`. See the
     * comment in the script block — `absolute` would offset by
     * whatever positioned ancestor we end up inside. */
    position: fixed;
    z-index: 11;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 3px;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.18);
    user-select: none;
  }

  .popup-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text);
    cursor: pointer;
  }

  .popup-btn:hover {
    background: var(--link-bg);
    color: var(--link);
  }

  .popup-btn:focus-visible {
    outline: 2px solid var(--link);
    outline-offset: -2px;
  }
</style>
