//! Tests for the copy-as-plain-text translator. The handler runs
//! on every copy event in the page, so its decision about whether
//! a selection counts as "a diff selection" needs to be precise —
//! getting it wrong breaks copy in unrelated parts of the UI.

import { afterEach, beforeEach, describe, expect, test } from 'vitest';
import { plainTextForSelection } from './diffCopy';

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
