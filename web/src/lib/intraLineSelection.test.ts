//! Tests for the DOM-selection → character-offset translator that
//! the intra-line comment composer relies on. The helper lives at
//! the boundary between the browser's `Range` API and the
//! `(side, line, columns)` shape that gets persisted, so a quiet
//! regression here either suppresses the "Comment on selection"
//! pill or — worse — sends the wrong offsets to the backend. These
//! tests exercise the contract end-to-end through jsdom.

import { afterEach, beforeEach, describe, expect, test } from 'vitest';
import { intraLineSelectionFor } from './intraLineSelection';

// jsdom doesn't ship `Range.getBoundingClientRect`. The helper uses
// it for pill positioning — exact values don't matter in unit tests,
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

describe('intraLineSelectionFor', () => {
  test('returns null when there is no selection', () => {
    expect(intraLineSelectionFor(table)).toBeNull();
  });

  test('returns null for a collapsed (zero-width) selection', () => {
    select(42, 5, 5);
    expect(intraLineSelectionFor(table)).toBeNull();
  });

  test('resolves a selection inside one line to side+line+offsets', () => {
    // The cell text is "function foo() {}" — selecting "foo" should
    // come back as offsets 9..12 on side=tip, line=42.
    select(42, 9, 12);
    const sel = intraLineSelectionFor(table);
    expect(sel).not.toBeNull();
    expect(sel!.side).toBe('tip');
    expect(sel!.line).toBe(42);
    expect(sel!.startOffset).toBe(9);
    expect(sel!.endOffset).toBe(12);
  });

  test('handles selections that span syntax-highlight span boundaries', () => {
    // The cell splits "function" and "foo" into separate spans plus
    // a bare " " text node. A selection from inside one span to
    // inside another should still report contiguous offsets — the
    // helper synthesises a Range against the cell root so the
    // intermediate span structure is invisible.
    select(42, 4, 12); // "tion foo"
    const sel = intraLineSelectionFor(table)!;
    expect(sel.startOffset).toBe(4);
    expect(sel.endOffset).toBe(12);
  });

  test('returns null when the selection spans two rows', () => {
    // Multi-row selections can't be represented as a single column
    // range — the production caller falls back to line-level. The
    // helper signals "not for me" by returning null.
    const cellA = table.querySelector<HTMLElement>(`[data-line="42"]`)!;
    const cellB = table.querySelector<HTMLElement>(`[data-line="43"]`)!;
    const aStart = locate(cellA, 0);
    const bEnd = locate(cellB, 3);
    const range = document.createRange();
    range.setStart(aStart.node, aStart.offset);
    range.setEnd(bEnd.node, bEnd.offset);
    const sel = window.getSelection()!;
    sel.removeAllRanges();
    sel.addRange(range);
    expect(intraLineSelectionFor(table)).toBeNull();
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
    expect(intraLineSelectionFor(table)).toBeNull();
  });

  test('includes the selection rect for pill positioning', () => {
    // jsdom returns zero-rects from getBoundingClientRect, but the
    // shape must still be a DOMRect (not undefined) so the caller's
    // positioning code never has to null-check.
    select(42, 9, 12);
    const sel = intraLineSelectionFor(table)!;
    expect(sel.rect).toBeDefined();
    expect(typeof sel.rect.top).toBe('number');
    expect(typeof sel.rect.left).toBe('number');
  });
});
