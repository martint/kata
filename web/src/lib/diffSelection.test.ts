//! Tests for the DOM-selection → character-offset translator that
//! the selection-driven comment popup relies on. The helper lives at
//! the boundary between the browser's `Range` API and the
//! `(side, lines, columns)` shape that gets persisted, so a quiet
//! regression here either suppresses the popup or — worse — sends
//! the wrong offsets to the backend. These tests exercise the
//! contract end-to-end through jsdom.

import { afterEach, beforeEach, describe, expect, test } from 'vitest';
import { diffSelectionFor } from './diffSelection';

// jsdom doesn't ship `Range.getBoundingClientRect`. The helper uses
// it for popup positioning — exact values don't matter in unit tests,
// just that the call succeeds. Polyfill once at module load.
if (typeof Range.prototype.getBoundingClientRect !== 'function') {
  Range.prototype.getBoundingClientRect = function () {
    return { top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({}) } as DOMRect;
  };
}

let table: HTMLTableElement;

beforeEach(() => {
  // Minimal table mirroring the structure HunkLines / SBS render:
  // a `.content` cell carries `data-side` + `data-line`, with text
  // (optionally nested in spans, mimicking syntax highlights).
  document.body.innerHTML = `
    <table>
      <tbody>
        <tr>
          <td class="content" data-side="tip" data-line="42">
            <pre><span style="color:red">function</span> <span style="color:blue">foo</span>() {}</pre>
          </td>
        </tr>
        <tr>
          <td class="content" data-side="tip" data-line="43">
            <pre>return 1;</pre>
          </td>
        </tr>
        <tr>
          <td class="content" data-side="tip" data-line="44">
            <pre>}</pre>
          </td>
        </tr>
        <tr>
          <td class="content" data-side="base" data-line="50">
            <pre>old line</pre>
          </td>
        </tr>
      </tbody>
    </table>
  `;
  table = document.querySelector('table')!;
});

afterEach(() => {
  window.getSelection()?.removeAllRanges();
  document.body.innerHTML = '';
});

/** Select `[start, end)` characters inside the `.content` cell for
 *  the given line. Walks the cell's text nodes to find the right
 *  (textNode, offset) pair for each endpoint — same arithmetic as
 *  the production helper does in reverse. */
function select(line: number, start: number, end: number) {
  const cell = table.querySelector<HTMLElement>(`[data-line="${line}"]`)!;
  const { node: startNode, offset: startOffset } = locate(cell, start);
  const { node: endNode, offset: endOffset } = locate(cell, end);
  const range = document.createRange();
  range.setStart(startNode, startOffset);
  range.setEnd(endNode, endOffset);
  const sel = window.getSelection()!;
  sel.removeAllRanges();
  sel.addRange(range);
}

/** Select from `(startLine, startCol)` to `(endLine, endCol)` —
 *  the multi-line analogue of `select`. */
function selectAcross(
  startLine: number,
  startCol: number,
  endLine: number,
  endCol: number,
) {
  const a = table.querySelector<HTMLElement>(`[data-line="${startLine}"]`)!;
  const b = table.querySelector<HTMLElement>(`[data-line="${endLine}"]`)!;
  const start = locate(a, startCol);
  const end = locate(b, endCol);
  const range = document.createRange();
  range.setStart(start.node, start.offset);
  range.setEnd(end.node, end.offset);
  const sel = window.getSelection()!;
  sel.removeAllRanges();
  sel.addRange(range);
}

function locate(cell: HTMLElement, target: number): { node: Node; offset: number } {
  let remaining = target;
  const walker = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode();
  while (node) {
    const len = (node as Text).data.length;
    if (remaining <= len) {
      return { node, offset: remaining };
    }
    remaining -= len;
    node = walker.nextNode();
  }
  // Past the end of the cell — pin to the last text node.
  const last = cell.lastChild as Text;
  return { node: last, offset: (last as Text).data.length };
}

describe('diffSelectionFor', () => {
  test('returns null when there is no selection', () => {
    expect(diffSelectionFor(table)).toBeNull();
  });

  test('returns null for a collapsed (zero-width) selection', () => {
    select(42, 5, 5);
    expect(diffSelectionFor(table)).toBeNull();
  });

  test('resolves a single-line selection to side+lines+cols', () => {
    // The cell text is "function foo() {}" — selecting "foo" should
    // come back as cols 9..12 on side=tip, lines 42..42.
    select(42, 9, 12);
    const sel = diffSelectionFor(table);
    expect(sel).not.toBeNull();
    expect(sel!.side).toBe('tip');
    expect(sel!.startLine).toBe(42);
    expect(sel!.endLine).toBe(42);
    expect(sel!.startCol).toBe(9);
    expect(sel!.endCol).toBe(12);
    expect(sel!.multiLine).toBe(false);
  });

  test('handles selections that span syntax-highlight span boundaries', () => {
    // The cell splits "function" and "foo" into separate spans plus
    // a bare " " text node. A selection from inside one span to
    // inside another should still report contiguous offsets — the
    // helper synthesises a Range against the cell root so the
    // intermediate span structure is invisible.
    select(42, 4, 12); // "tion foo"
    const sel = diffSelectionFor(table)!;
    expect(sel.startCol).toBe(4);
    expect(sel.endCol).toBe(12);
  });

  test('resolves a multi-line selection to side+lines+cols', () => {
    // From col 4 on line 42 to col 6 on line 43 — popup-driven
    // free-form selection. Start col on first line, end col on last;
    // no relation enforced between the two.
    selectAcross(42, 4, 43, 6);
    const sel = diffSelectionFor(table)!;
    expect(sel.side).toBe('tip');
    expect(sel.startLine).toBe(42);
    expect(sel.endLine).toBe(43);
    expect(sel.startCol).toBe(4);
    expect(sel.endCol).toBe(6);
    expect(sel.multiLine).toBe(true);
  });

  test('allows multi-line selection where end col < start col', () => {
    // First line has "function foo() {}" (17 chars), last line is
    // just "}" (1 char). Selecting from col 10 on line 42 to col 1
    // on line 44 is well-formed multi-line — the helper must NOT
    // apply the single-line `endCol > startCol` guard here.
    selectAcross(42, 10, 44, 1);
    const sel = diffSelectionFor(table)!;
    expect(sel.startLine).toBe(42);
    expect(sel.endLine).toBe(44);
    expect(sel.startCol).toBe(10);
    expect(sel.endCol).toBe(1);
    expect(sel.multiLine).toBe(true);
  });

  test('returns null when the selection spans mixed sides', () => {
    // base/tip mix can't anchor a single comment — neither side has
    // a coherent (file, line, side) tuple. The caller suppresses the
    // popup; the helper signals that by returning null.
    selectAcross(43, 0, 50, 3);
    expect(diffSelectionFor(table)).toBeNull();
  });

  test('returns null when the selection is outside the table', () => {
    // A selection inside an unrelated DOM subtree (e.g. the user
    // dragged text in the page header) must not be misattributed to
    // the diff. Guards against the bound argument doing real work.
    document.body.insertAdjacentHTML('beforeend', '<p id="outside">hello world</p>');
    const outside = document.getElementById('outside')!;
    const range = document.createRange();
    range.setStart(outside.firstChild!, 0);
    range.setEnd(outside.firstChild!, 5);
    const sel = window.getSelection()!;
    sel.removeAllRanges();
    sel.addRange(range);
    expect(diffSelectionFor(table)).toBeNull();
  });

  test('includes the selection rect for popup positioning', () => {
    // jsdom returns zero-rects from getBoundingClientRect, but the
    // shape must still be a DOMRect (not undefined) so the caller's
    // positioning code never has to null-check.
    select(42, 9, 12);
    const sel = diffSelectionFor(table)!;
    expect(sel.rect).toBeDefined();
    expect(typeof sel.rect.top).toBe('number');
    expect(typeof sel.rect.left).toBe('number');
  });
});
