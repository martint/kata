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
    /** Open the comment composer anchored to the selection's line
     *  range (and column range for multi-line). */
    oncomment: () => void;
    /** Copy the selected text to the clipboard as plain text. */
    oncopy: () => void;
    /** Copy a permalink URL pointing at the selection's line range
     *  to the clipboard. */
    onpermalink: () => void;
  }
  const { selection, oncomment, oncopy, onpermalink }: Props = $props();

  // Document-coord positioning (page scroll added) since the popup is
  // absolutely positioned at the page level and selections come from
  // viewport-relative DOMRects.
  const top = $derived(selection ? selection.rect.bottom + window.scrollY + 4 : 0);
  const left = $derived(selection ? selection.rect.right + window.scrollX + 4 : 0);
</script>

{#if selection}
  <div
    class="selection-popup"
    role="toolbar"
    aria-label="Selection actions"
    tabindex="-1"
    style:top="{top}px"
    style:left="{left}px"
    onmousedown={(e) => {
      // Mousedown on the popup itself shouldn't clear the underlying
      // text selection (which the parent's dismissal handler keys
      // off). `preventDefault` keeps the browser from collapsing the
      // selection on the click; the parent's handler also short-
      // circuits when the click target is inside `.selection-popup`.
      e.preventDefault();
    }}
  >
    <button
      type="button"
      class="popup-btn"
      title="Comment on selection"
      aria-label="Comment on selection"
      onclick={oncomment}
    >
      <Bubble size={14} />
    </button>
    <button
      type="button"
      class="popup-btn"
      title="Copy selected text"
      aria-label="Copy selected text"
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
    position: absolute;
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
