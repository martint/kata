//! Rewrite the clipboard payload when the user copies a selection
//! inside a diff.
//!
//! The diff renders as an HTML `<table>` so the gutter (line number
//! + +comment button) can sit in a sticky column next to the
//! content. Browsers default-copy table selections as `text/html`
//! containing the full `<table><tr><td>...` markup, which pastes
//! into other apps as a table — useless for sharing a snippet of
//! code with someone. We intercept `copy` events, rebuild a
//! plain-text version (the joined text of the selected `.content`
//! cells, one per row), and replace the clipboard payload.
//!
//! Selections that don't touch a `.content[data-side]` cell pass
//! through untouched, so copying from the comment composer, the
//! sidebar, or anywhere else outside the diff still works normally.

import type { DiffSelection } from './diffSelection';

/** Install the document-level copy handler. Idempotent — calling
 *  twice during HMR is harmless. Returns an unsubscribe for tests
 *  and for the (currently unused) component-teardown path. */
export function installDiffCopyHandler(): () => void {
  if (typeof document === 'undefined') return () => {};
  const onCopy = (e: ClipboardEvent) => {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;
    const range = sel.getRangeAt(0);
    const text = plainTextForSelection(range);
    if (text == null) return;
    e.preventDefault();
    // Set both text/plain (target most apps) and an empty text/html
    // override — without the latter Chrome falls back to its own
    // serialiser and we'd still get a table.
    e.clipboardData?.setData('text/plain', text);
    e.clipboardData?.setData('text/html', escapeHtml(text));
  };
  document.addEventListener('copy', onCopy);
  return () => document.removeEventListener('copy', onCopy);
}

/** Build a plain-text copy string from `range` if any `.content`
 *  diff cell intersects it. `null` means "not a diff selection,
 *  let the browser handle it." */
export function plainTextForSelection(range: Range): string | null {
  const start = ancestorElement(range.commonAncestorContainer);
  if (!start) return null;
  // `querySelectorAll` doesn't include the root itself, so when the
  // selection lives inside ONE cell the root IS that cell and the
  // query comes back empty. Walk up to a non-cell ancestor (the
  // surrounding table is plenty) and query from there.
  const root = start.closest('.content[data-side]')
    ? start.closest('.content[data-side]')!.parentElement ?? start
    : start;
  const cells = root.querySelectorAll<HTMLElement>('.content[data-side]');
  const lines: string[] = [];
  let saw = false;
  for (const cell of cells) {
    if (!range.intersectsNode(cell)) continue;
    saw = true;
    lines.push(textForCellWithin(cell, range));
  }
  if (!saw) return null;
  return lines.join('\n');
}

/** Text of `cell`'s content clipped to `range`'s bounds. For cells
 *  fully inside the selection this is the whole cell; for the start
 *  and end cells it's the partial slice the user selected. */
function textForCellWithin(cell: HTMLElement, range: Range): string {
  const cellRange = document.createRange();
  cellRange.selectNodeContents(cell);
  // Tighten the start to the selection's start if that boundary
  // lies inside this cell.
  if (
    cell.contains(range.startContainer) &&
    range.compareBoundaryPoints(Range.START_TO_START, cellRange) > 0
  ) {
    cellRange.setStart(range.startContainer, range.startOffset);
  }
  // Same for the end.
  if (
    cell.contains(range.endContainer) &&
    range.compareBoundaryPoints(Range.END_TO_END, cellRange) < 0
  ) {
    cellRange.setEnd(range.endContainer, range.endOffset);
  }
  // `Range.toString()` returns the visible text, which is exactly
  // what we want — it walks the cell's `<pre>` honouring whitespace
  // and skips any nested word-diff / column-anchor span boundaries.
  return cellRange.toString();
}

function ancestorElement(node: Node): Element | null {
  return node.nodeType === Node.ELEMENT_NODE
    ? (node as Element)
    : node.parentElement;
}

/** Build the clipboard text for a `DiffSelection`, splicing in the
 *  underlying source for any line in `[startLine, endLine]` whose
 *  cell isn't rendered — inter-hunk gaps in particular. Used both
 *  by the selection-popup's Copy button (where the caller awaits a
 *  fetch of `fileText` first) and by the per-file Ctrl+C handler
 *  (where `fileText` may still be null on the very first drag of a
 *  file; hidden lines fall through to empty strings rather than
 *  emit stale rendered text).
 *
 *  - `wrapper` — the file's `.hunks-wrapper`. Used to look up
 *    rendered `td.content[data-side][data-line]` cells.
 *  - `sel` — the resolved selection: side, line range, column
 *    range. First / last line are clipped to `startCol` / `endCol`;
 *    middle lines copy in full.
 *  - `fileText` — the full file content for `sel.side`, or `null`
 *    if the fetch hasn't resolved (or failed) yet. `null` causes
 *    hidden lines to emit `''`; the caller is responsible for
 *    making sure the content is loaded when it cares about the
 *    hidden-line text. */
export function buildCopyText(
  wrapper: HTMLElement,
  sel: DiffSelection,
  fileText: string | null,
): string {
  const fileLines = fileText != null ? fileText.split('\n') : null;
  const lines: string[] = [];
  for (let ln = sel.startLine; ln <= sel.endLine; ln++) {
    const pre = wrapper.querySelector(
      `td.content[data-side="${sel.side}"][data-line="${ln}"] pre`,
    ) as HTMLElement | null;
    let text: string;
    if (pre) {
      text = pre.textContent ?? '';
    } else if (fileLines) {
      // 1-indexed line numbers; clamp to defend against off-by-one
      // at EOF (file with no trailing newline yields one fewer
      // entry than the last line number we'd expect).
      text = fileLines[ln - 1] ?? '';
    } else {
      text = '';
    }
    if (ln === sel.startLine && ln === sel.endLine) {
      text = text.slice(sel.startCol, sel.endCol);
    } else if (ln === sel.startLine) {
      text = text.slice(sel.startCol);
    } else if (ln === sel.endLine) {
      text = text.slice(0, sel.endCol);
    }
    lines.push(text);
  }
  return lines.join('\n');
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}
