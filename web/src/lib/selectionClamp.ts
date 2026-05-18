//! Clamp the browser's native text selection to the file's hunks
//! wrapper — a drag that begins inside one file's diff can't extend
//! into a different file. Within a single file, selections may span
//! across hunks (the comment composer anchors the implied range to
//! every line from start to end, including any hidden inter-hunk
//! lines).
//!
//! On mouseup, if the Range has escaped the wrapper, shrink its
//! endpoints to the first / last selectable text inside the wrapper.
//! No user-select toggling during the drag — the diff's CSS makes
//! the browser's native `::selection` paint transparent inside
//! `.hunks` anyway, so cross-file extension is invisible. The
//! mouseup clamp only matters for the final logical selection
//! (what the popup / overlay code reads).

/** Install the clamp on `root`. Mousedown is listened to on `root`
 *  so drags starting outside the diff don't activate clamping; the
 *  mouseup listener runs on `document` because the pointer may
 *  release outside `root` once the user has dragged off-axis.
 *
 *  Returns a cleanup function — call it on component unmount. */
export function installSelectionClamp(root: HTMLElement): () => void {
  /** True between a mousedown that originated inside `root` and the
   *  following mouseup. Mouseup is only allowed to clamp the
   *  selection if the drag actually started here — clicks elsewhere
   *  don't get to truncate someone else's selection. */
  let active = false;

  function onMouseDown(e: MouseEvent) {
    const t = e.target as Element | null;
    if (!t) return;
    // Skip activation for clicks on interactive elements — buttons,
    // chevrons, etc. They're not the start of a text drag.
    if (t.closest('button')) return;
    if (!root.contains(t)) return;
    active = true;
  }

  function onMouseUp() {
    if (!active) return;
    active = false;
    const sel = typeof window !== 'undefined' ? window.getSelection() : null;
    if (!sel || sel.rangeCount === 0) return;
    const range = sel.getRangeAt(0);
    const startIn = root.contains(range.startContainer);
    const endIn = root.contains(range.endContainer);
    if (startIn && endIn) return;
    const newRange = range.cloneRange();
    if (!startIn) {
      const first = findFirstSelectableText(root);
      if (first) newRange.setStart(first, 0);
    }
    if (!endIn) {
      const last = findLastSelectableText(root);
      if (last) newRange.setEnd(last, last.data.length);
    }
    sel.removeAllRanges();
    sel.addRange(newRange);
  }

  root.addEventListener('mousedown', onMouseDown);
  document.addEventListener('mouseup', onMouseUp);

  return () => {
    root.removeEventListener('mousedown', onMouseDown);
    document.removeEventListener('mouseup', onMouseUp);
    active = false;
  };
}

/** First text node inside the first `.content > pre` descendant of
 *  `root` (typically a single hunk's table or the whole hunks-
 *  wrapper). Scoped to `.content > pre` so we skip between-tag
 *  whitespace and line-number cells. */
export function findFirstSelectableText(root: Element): Text | null {
  const cells = root.querySelectorAll('td.content');
  for (const cell of cells) {
    const t = firstTextInPre(cell);
    if (t) return t;
  }
  return null;
}

/** Last text node inside the last `.content > pre` descendant of
 *  `root`. Walk cells in reverse so we don't have to traverse the
 *  whole tree just to find the tail. */
export function findLastSelectableText(root: Element): Text | null {
  const cells = root.querySelectorAll('td.content');
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
