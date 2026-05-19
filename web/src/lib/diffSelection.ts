//! Helpers for translating a browser text selection inside a diff
//! into the `(file, side, lines, columns)` tuple a comment composer
//! needs. Handles both single-line selections (the "drag a few
//! characters" case the intra-line composer originally shipped for)
//! and multi-line selections (drag from mid-line on one row to mid-
//! line on a later row — the shape the selection-driven popup
//! anchors free-form comments against).
//!
//! Browser selections are described by `Range` objects whose
//! `startContainer` / `endContainer` are text nodes nested somewhere
//! inside the highlighted-HTML span tree. The renderer doesn't lay
//! its own offsets next to each character; instead we re-derive them
//! by asking the browser "how many characters are between the cell
//! root and this point" via a synthetic `Range`. The browser's
//! `Range.toString()` returns the visible text, which matches the
//! original line text the highlighter was given.

/** A successfully resolved diff-selection. Offsets are UTF-16,
 *  half-open `[startCol, endCol)` for single-line, otherwise
 *  `startCol` is the offset on the first selected line and `endCol`
 *  is the offset on the last (no relation between the two values —
 *  the last line may end well before the first line's start col). */
export interface DiffSelection {
  /** `'base'` or `'tip'` — copied from the cell's `data-side`. Every
   *  selected `.content` cell must share this side; mixed-side
   *  selections (added + removed in unified mode, or base-side +
   *  tip-side in SBS) return `null` from the resolver. */
  side: 'base' | 'tip';
  /** 1-based line of the first selected `.content` cell. */
  startLine: number;
  /** 1-based line of the last selected `.content` cell. May equal
   *  `startLine` for a single-line selection. */
  endLine: number;
  /** Character offset of the selection's start within the FIRST
   *  cell's text. */
  startCol: number;
  /** Character offset of the selection's end within the LAST cell's
   *  text. */
  endCol: number;
  /** Convenience: true when the selection covers more than one line.
   *  Equivalent to `startLine !== endLine`. */
  multiLine: boolean;
  /** The selection rect in viewport coordinates — caller positions
   *  the floating selection popup against it. */
  rect: DOMRect;
}

/** Inspect the current window selection and, if it's a non-empty
 *  range whose start AND end cells live inside `root`, return the
 *  corresponding diff selection. Returns `null` when:
 *
 *  - There is no selection or it is collapsed (zero-width).
 *  - Either endpoint is outside a `.content` cell.
 *  - Either endpoint's cell isn't inside `root` (selection started
 *    in another component / outside the diff).
 *  - The two endpoint cells live on different sides (`base` vs
 *    `tip`) — a mixed-side selection has no coherent comment anchor,
 *    so the caller falls back to leaving the popup hidden.
 *
 *  Notably the function does NOT reject multi-line selections — that's
 *  the whole point of generalising past the original single-cell
 *  intra-line helper. */
export function diffSelectionFor(root: HTMLElement): DiffSelection | null {
  if (typeof window === 'undefined') return null;
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
  const range = sel.getRangeAt(0);
  const startCell = closestContentCell(range.startContainer, root);
  const endCell = closestContentCell(range.endContainer, root);
  if (!startCell || !endCell) return null;

  const startSide = startCell.getAttribute('data-side');
  const endSide = endCell.getAttribute('data-side');
  if ((startSide !== 'base' && startSide !== 'tip') || startSide !== endSide) {
    return null;
  }
  const side = startSide;

  const startLineStr = startCell.getAttribute('data-line');
  const endLineStr = endCell.getAttribute('data-line');
  if (!startLineStr || !endLineStr) return null;
  const startLine = Number(startLineStr);
  const endLine = Number(endLineStr);
  if (
    !Number.isFinite(startLine) ||
    startLine <= 0 ||
    !Number.isFinite(endLine) ||
    endLine <= 0
  ) {
    return null;
  }
  // Backward-direction selections (user dragged up) come through with
  // the same start/end nodes regardless of drag direction — the
  // browser normalises so `range.startContainer` is the
  // document-order earlier node. Defensive belt-and-braces: enforce
  // here too in case a future renderer breaks that assumption.
  if (endLine < startLine) return null;

  const startCol = offsetWithin(startCell, range.startContainer, range.startOffset);
  const endCol = offsetWithin(endCell, range.endContainer, range.endOffset);

  // Single-line case still needs a positive-width slice — otherwise
  // it's a click that happened to register as a zero-char selection
  // (e.g. between two glyphs), which the caller should treat as
  // "nothing selected".
  if (startLine === endLine && endCol <= startCol) return null;

  const rect = range.getBoundingClientRect();
  return {
    side,
    startLine,
    endLine,
    startCol,
    endCol,
    multiLine: startLine !== endLine,
    rect,
  };
}

/** Walk up from `node` and return the nearest ancestor `.content`
 *  cell, but only if it sits inside `bound` — keeps selections that
 *  started outside the table from being misattributed. */
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
