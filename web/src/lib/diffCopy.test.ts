//! Tests for the copy-as-plain-text translator. The handler runs
//! on every copy event in the page, so its decision about whether
//! a selection counts as "a diff selection" needs to be precise —
//! getting it wrong breaks copy in unrelated parts of the UI.

import { afterEach, beforeEach, describe, expect, test } from 'vitest';
import { buildCopyText, plainTextForSelection } from './diffCopy';
import type { DiffSelection } from './diffSelection';

function selectAll(root: Element): Range {
  const range = document.createRange();
  range.selectNodeContents(root);
  return range;
}

/** Build a minimal diff-shaped DOM: a table with two rows, each
 *  with a `.content[data-side]` cell holding a `<pre>`. Matches
 *  what HunkLines renders (without the gutter, since the gutter
 *  has `user-select: none` and never enters selections). */
function mountDiff() {
  document.body.innerHTML = `
    <table>
      <tbody>
        <tr><td class="content" data-side="tip" data-line="1"><pre>function foo() {</pre></td></tr>
        <tr><td class="content" data-side="tip" data-line="2"><pre>  return 42;</pre></td></tr>
        <tr><td class="content" data-side="tip" data-line="3"><pre>}</pre></td></tr>
      </tbody>
    </table>
  `;
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('plainTextForSelection', () => {
  beforeEach(() => mountDiff());

  test('joins each selected diff content cell with newlines', () => {
    const range = selectAll(document.querySelector('table')!);
    expect(plainTextForSelection(range)).toBe(
      'function foo() {\n  return 42;\n}',
    );
  });

  test('clips the start cell to the selection start offset', () => {
    // Select from offset 9 of the first <pre> through end of the
    // second <pre> — should yield "foo() {\n  return 42;".
    const first = document.querySelectorAll('pre')[0].firstChild as Text;
    const second = document.querySelectorAll('pre')[1].firstChild as Text;
    const range = document.createRange();
    range.setStart(first, 9);
    range.setEnd(second, second.data.length);
    expect(plainTextForSelection(range)).toBe('foo() {\n  return 42;');
  });

  test('returns null for selections that touch no diff content cell', () => {
    // Put a paragraph outside the table; selecting it should pass
    // through to the browser's default copy behaviour.
    document.body.insertAdjacentHTML(
      'beforeend',
      '<p id="outside">unrelated text</p>',
    );
    const outside = document.getElementById('outside')!;
    const range = selectAll(outside);
    expect(plainTextForSelection(range)).toBeNull();
  });

  test('a single-cell selection comes back as just that cell', () => {
    const cell = document.querySelectorAll('.content')[1]!;
    const range = selectAll(cell);
    expect(plainTextForSelection(range)).toBe('  return 42;');
  });

  test('an empty selection inside a diff cell still returns the cell text', () => {
    // selectNodeContents on a pre with one child gives a non-collapsed
    // range covering the text. Confirms the function doesn't filter
    // out zero-length text on accident.
    const cell = document.querySelectorAll('.content')[2]!;
    const range = selectAll(cell);
    expect(plainTextForSelection(range)).toBe('}');
  });
});

function sel(over: Partial<DiffSelection>): DiffSelection {
  return {
    side: 'tip',
    startLine: 1,
    endLine: 1,
    startCol: 0,
    endCol: 0,
    multiLine: false,
    rect: new DOMRect(),
    ...over,
  };
}

describe('buildCopyText', () => {
  /** A two-hunk file: lines 10–11 rendered, lines 12–19 hidden in
   *  an inter-hunk gap, line 20 rendered again. Models the most
   *  interesting case — a multi-hunk selection that crosses the
   *  gap — and lets us assert both the rendered and the hidden
   *  branches. */
  function mountTwoHunks() {
    document.body.innerHTML = `
      <div class="wrapper">
        <table>
          <tbody>
            <tr><td class="content" data-side="tip" data-line="10"><pre>first line</pre></td></tr>
            <tr><td class="content" data-side="tip" data-line="11"><pre>second line</pre></td></tr>
          </tbody>
        </table>
        <table>
          <tbody>
            <tr><td class="content" data-side="tip" data-line="20"><pre>far line</pre></td></tr>
          </tbody>
        </table>
      </div>
    `;
    return document.querySelector('.wrapper') as HTMLElement;
  }

  // 1-indexed source content for the tip side. The rendered lines
  // (10, 11, 20) match the DOM cells above; lines 12–19 are the
  // hidden inter-hunk gap and exist only here.
  const tipLines: string[] = [];
  for (let i = 1; i <= 20; i++) {
    if (i === 10) tipLines.push('first line');
    else if (i === 11) tipLines.push('second line');
    else if (i >= 12 && i <= 19)
      tipLines.push(`gap line ${String.fromCharCode(96 + i - 11)}`);
    else if (i === 20) tipLines.push('far line');
    else tipLines.push(`line ${i}`);
  }
  const tipText = tipLines.join('\n');

  test('returns the rendered cell text for a single-line range with no column clip', () => {
    const wrapper = mountTwoHunks();
    expect(
      buildCopyText(
        wrapper,
        sel({ startLine: 10, endLine: 10, startCol: 0, endCol: 10 }),
        null,
      ),
    ).toBe('first line');
  });

  test('clips first / last lines to startCol / endCol, full text in between', () => {
    const wrapper = mountTwoHunks();
    // Line 10 'first line' → slice(6) = 'line'.
    // Line 11 'second line' → slice(0, 6) = 'second'.
    expect(
      buildCopyText(
        wrapper,
        sel({ startLine: 10, endLine: 11, startCol: 6, endCol: 6 }),
        null,
      ),
    ).toBe('line\nsecond');
  });

  test('splices hidden inter-hunk lines from the cached file text', () => {
    // Lines 10 (rendered) → 20 (rendered) crosses the 12–19 gap.
    // Without the cache we'd lose those lines entirely; with it
    // the helper recovers them from `tipText`.
    const wrapper = mountTwoHunks();
    const out = buildCopyText(
      wrapper,
      sel({ startLine: 10, endLine: 20, startCol: 0, endCol: 8 }),
      tipText,
    );
    expect(out.split('\n')).toEqual([
      'first line',
      'second line',
      'gap line a',
      'gap line b',
      'gap line c',
      'gap line d',
      'gap line e',
      'gap line f',
      'gap line g',
      'gap line h',
      'far line',
    ]);
  });

  test('emits empty strings for hidden lines when fileText is null', () => {
    // The popup's Copy button awaits the fetch, but the Ctrl+C
    // path may race the fetch on the very first drag — we never
    // want to fabricate text from a rendered cell that isn't
    // there. Hidden lines come back as empty placeholders.
    const wrapper = mountTwoHunks();
    const out = buildCopyText(
      wrapper,
      sel({ startLine: 10, endLine: 20, startCol: 0, endCol: 8 }),
      null,
    );
    const lines = out.split('\n');
    // Rendered ends visible, gap is blank.
    expect(lines[0]).toBe('first line');
    expect(lines[1]).toBe('second line');
    for (let i = 2; i <= 9; i++) expect(lines[i]).toBe('');
    expect(lines[10]).toBe('far line');
  });

  test('a single-line slice with both clips returns the clipped substring only', () => {
    // 'second line' chars [2, 8) = 'cond l'.
    const wrapper = mountTwoHunks();
    expect(
      buildCopyText(
        wrapper,
        sel({ startLine: 11, endLine: 11, startCol: 2, endCol: 8 }),
        null,
      ),
    ).toBe('cond l');
  });

  test('reads from the base-side cells when sel.side is base', () => {
    document.body.innerHTML = `
      <div class="wrapper">
        <table>
          <tbody>
            <tr><td class="content" data-side="base" data-line="5"><pre>base line</pre></td></tr>
          </tbody>
        </table>
      </div>
    `;
    const wrapper = document.querySelector('.wrapper') as HTMLElement;
    expect(
      buildCopyText(
        wrapper,
        sel({ side: 'base', startLine: 5, endLine: 5, startCol: 0, endCol: 9 }),
        null,
      ),
    ).toBe('base line');
  });
});
