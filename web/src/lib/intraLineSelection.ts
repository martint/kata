//! Helpers for translating a browser text selection inside a diff's
//! `.content` cell into the (file, side, line, columns) tuple a
//! line-level comment composer needs.
//!
//! Browser selections are described by `Range` objects whose
//! `startContainer` / `endContainer` are text nodes nested somewhere
//! inside the highlighted-HTML span tree. The renderer doesn't lay
//! its own offsets next to each character; instead we re-derive them
//! by asking the browser "how many characters are between the cell
//! root and this point" via a synthetic `Range`. The browser's
//! `Range.toString()` returns the visible text, which matches the
//! original line text the highlighter was given.

/** A character-range selection successfully resolved against one
 *  diff row. Offsets are UTF-16, half-open `[start, end)`. */
export interface IntraLineSelection {
  /** `'base'` or `'tip'` — copied from the cell's `data-side`. */
  side: 'base' | 'tip';
  /** 1-based line number — copied from `data-line`. */
  line: number;
  /** Character offset of the selection's start within the cell text. */
  startOffset: number;
  /** Exclusive end offset. */
  endOffset: number;
  /** The selection rect in viewport coordinates — caller positions
   *  any floating "Comment on selection" affordance against it. */
  rect: DOMRect;
}

/** Inspect the current window selection and, if it's a non-empty
 *  range fully inside a single `.content` cell of `table`, return
 *  the corresponding intra-line selection. Returns `null` when
 *  there is no selection, the selection is empty, it isn't inside
 *  `table`, or it spans more than one row. */
export function intraLineSelectionFor(table: HTMLElement): IntraLineSelection | null {
  if (typeof window === 'undefined') return null;
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
  const range = sel.getRangeAt(0);
  const startCell = closestContentCell(range.startContainer, table);
  const endCell = closestContentCell(range.endContainer, table);
  if (!startCell || startCell !== endCell) return null;

  const side = startCell.getAttribute('data-side');
  const lineStr = startCell.getAttribute('data-line');
  if ((side !== 'base' && side !== 'tip') || !lineStr) return null;
  const line = Number(lineStr);
  if (!Number.isFinite(line) || line <= 0) return null;

  const startOffset = offsetWithin(startCell, range.startContainer, range.startOffset);
  const endOffset = offsetWithin(startCell, range.endContainer, range.endOffset);
  if (endOffset <= startOffset) return null;

  const rect = range.getBoundingClientRect();
  return { side, line, startOffset, endOffset, rect };
}

/** Walk up from `node` and return the nearest ancestor `.content`
 *  cell, but only if it sits inside `bound` (we don't want to leak
 *  selections that started outside the table). */
function closestContentCell(node: Node | null, bound: HTMLElement): HTMLElement | null {
  let cur: Node | null = node;
  while (cur && cur !== bound) {
    if (cur.nodeType === Node.ELEMENT_NODE) {
      const el = cur as HTMLElement;
      if (el.classList.contains('content')) return el;
    }
    cur = cur.parentNode;
  }
  return null;
}

/** Number of UTF-16 characters between `cell`'s start and the
 *  `(node, offset)` boundary. We synthesise a temporary `Range`
 *  rather than walking text nodes by hand because the browser
 *  already knows how its own DOM serialises to text — including
 *  trim/collapse rules for `<pre>` content, which the diff renders
 *  inside. */
function offsetWithin(cell: HTMLElement, node: Node, offset: number): number {
  const r = document.createRange();
  r.setStart(cell, 0);
  r.setEnd(node, offset);
  return r.toString().length;
}
