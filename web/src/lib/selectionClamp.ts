//! Clamp the browser's native text selection to the boundaries of
//! the `<table>` the drag started in. In a diff that means: a drag
//! that begins in one hunk's table can't extend into a neighbouring
//! hunk's table (unified mode), and a drag that begins on one SBS
//! side can't extend across the divider into the other side. Both
//! cases used to produce selections with no useful anchor — the
//! comment composer would silently reject them on mouseup, leaving
//! the reader confused about why nothing happened.
//!
//! Two-phase approach to avoid drag-loop jitter AND post-release
//! snap-out:
//!
//! 1. **During drag (CSS)** — on mousedown, the originating `<table>`
//!    is captured and every OTHER `<table>` in `root` gets an inline
//!    `user-select: none`. The browser refuses to PAINT the selection
//!    in those tables as the pointer drags through them; the visible
//!    highlight stops cleanly at the boundary. No JS in the drag loop
//!    means no flicker.
//!
//! 2. **On mouseup (range clamp)** — even though the browser doesn't
//!    paint the selection past the boundary, the underlying Range's
//!    endpoints still track the pointer through the unselectable
//!    regions. As soon as we restore `user-select`, those endpoints
//!    become visible — the selection appears to "snap out" to where
//!    the user released. Before restoring, we read the Range, shrink
//!    any out-of-bound endpoint to the corresponding edge of the
//!    bound table, and only then restore. The final visible selection
//!    matches what was painted during the drag.

/** Install the clamp on `root`. Mousedown is listened to on `root`
 *  so drags starting outside the diff don't activate clamping; the
 *  mouseup listener runs on `document` because the pointer may
 *  release outside `root` once the user has dragged off-axis.
 *
 *  Returns a cleanup function — call it on component unmount. */
export function installSelectionClamp(root: HTMLElement): () => void {
  /** Table the most recent mousedown happened inside — the one the
   *  selection is allowed to live in. */
  let dragStartTable: HTMLElement | null = null;
  /** Tables we set `user-select: none` on; remembered so mouseup
   *  restores exactly what we touched even if the DOM changed
   *  (hunks added/removed) in the meantime. */
  let suppressed: HTMLElement[] = [];

  function onMouseDown(e: MouseEvent) {
    const t = e.target as Element | null;
    if (!t) return;
    // Skip activation for clicks on interactive elements — buttons,
    // chevrons, etc. Flipping `user-select: none` on the sibling
    // tables would disturb any selection that's still alive in those
    // tables from a previous drag (the browser may collapse or
    // reflow the selection when its host element becomes
    // unselectable mid-drag). Buttons aren't the start of a text
    // drag, so there's nothing to clamp here.
    if (t.closest('button')) return;
    dragStartTable = (t.closest('table') as HTMLElement | null) ?? null;
    if (!dragStartTable) return;
    for (const tbl of root.querySelectorAll<HTMLElement>('table')) {
      if (tbl === dragStartTable) continue;
      tbl.style.userSelect = 'none';
      suppressed.push(tbl);
    }
  }

  function onMouseUp() {
    if (!dragStartTable) return;
    // Read the range and shrink any out-of-bound endpoint BEFORE
    // restoring user-select. If we restored first, the selection
    // would suddenly become visible across the previously-suppressed
    // tables — what the user reads as "selection snapped to where I
    // released the mouse."
    const sel = typeof window !== 'undefined' ? window.getSelection() : null;
    if (sel && sel.rangeCount > 0) {
      const range = sel.getRangeAt(0);
      const startIn = dragStartTable.contains(range.startContainer);
      const endIn = dragStartTable.contains(range.endContainer);
      if (!startIn || !endIn) {
        const newRange = range.cloneRange();
        if (!startIn) {
          const first = findFirstSelectableText(dragStartTable);
          if (first) newRange.setStart(first, 0);
        }
        if (!endIn) {
          const last = findLastSelectableText(dragStartTable);
          if (last) newRange.setEnd(last, last.data.length);
        }
        sel.removeAllRanges();
        sel.addRange(newRange);
      }
    }
    for (const tbl of suppressed) tbl.style.userSelect = '';
    suppressed = [];
    dragStartTable = null;
  }

  root.addEventListener('mousedown', onMouseDown);
  document.addEventListener('mouseup', onMouseUp);

  return () => {
    root.removeEventListener('mousedown', onMouseDown);
    document.removeEventListener('mouseup', onMouseUp);
    // If a drag was in progress at unmount time, leave no stuck
    // inline styles behind.
    for (const tbl of suppressed) tbl.style.userSelect = '';
    suppressed = [];
    dragStartTable = null;
  };
}

/** First text node inside the first `.content > pre` of `table`. The
 *  diff renders its code inside `<pre>` cells — between-tag
 *  whitespace text nodes (which a plain text walk would pick up)
 *  and line-number cells (which are `user-select: none`) are skipped
 *  by scoping to `.content > pre`. */
export function findFirstSelectableText(table: Element): Text | null {
  const cells = table.querySelectorAll('td.content');
  for (const cell of cells) {
    const t = firstTextInPre(cell);
    if (t) return t;
  }
  return null;
}

/** Last text node inside the last `.content > pre` of `table`. Walk
 *  cells in reverse so we don't have to traverse the whole tree just
 *  to find the tail. */
export function findLastSelectableText(table: Element): Text | null {
  const cells = table.querySelectorAll('td.content');
  for (let i = cells.length - 1; i >= 0; i--) {
    const t = lastTextInPre(cells[i]);
    if (t) return t;
  }
  return null;
}

function firstTextInPre(cell: Element): Text | null {
  const pre = cell.querySelector('pre');
  if (!pre) return null;
  const walker = document.createTreeWalker(pre, NodeFilter.SHOW_TEXT);
  return walker.nextNode() as Text | null;
}

function lastTextInPre(cell: Element): Text | null {
  const pre = cell.querySelector('pre');
  if (!pre) return null;
  // No reverse-walking API on TreeWalker — walk forward and remember
  // the last text node. Cells are small (one line of code), so this
  // is constant-time in practice.
  const walker = document.createTreeWalker(pre, NodeFilter.SHOW_TEXT);
  let last: Text | null = null;
  let next: Node | null;
  while ((next = walker.nextNode()) != null) last = next as Text;
  return last;
}
