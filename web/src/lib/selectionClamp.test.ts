//! Tests for the selection-clamp helpers. The full clamp lifecycle
//! (mousedown → drag → mouseup with range shrink) needs a real
//! browser to exercise — jsdom doesn't drive selection from pointer
//! events reliably — so we test the DOM-walking helpers in isolation.
//! Those carry the per-cell logic; the clamp itself is glue.

import { afterEach, beforeEach, describe, expect, test } from 'vitest';
import {
  findFirstSelectableText,
  findLastSelectableText,
} from './selectionClamp';

let table: HTMLTableElement;

beforeEach(() => {
  // Mirror the shape HunkLines / HunkLinesSideBySide render: rows
  // with one or two `td.ln` gutters and a `td.content > pre`. The
  // helpers scope to `.content > pre`, so gutter text and between-
  // tag whitespace should both be skipped.
  document.body.innerHTML = `
    <table>
      <tbody>
        <tr>
          <td class="ln">10</td>
          <td class="content" data-side="tip" data-line="10">
            <pre>first line</pre>
          </td>
        </tr>
        <tr>
          <td class="ln">11</td>
          <td class="content" data-side="tip" data-line="11">
            <pre>middle line</pre>
          </td>
        </tr>
        <tr>
          <td class="ln">12</td>
          <td class="content" data-side="tip" data-line="12">
            <pre>last line</pre>
          </td>
        </tr>
      </tbody>
    </table>
  `;
  table = document.querySelector('table')!;
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('findFirstSelectableText', () => {
  test('returns the first text node inside the first content pre', () => {
    const t = findFirstSelectableText(table);
    expect(t).not.toBeNull();
    expect(t!.data).toBe('first line');
  });

  test('skips gutter cells even if they have text', () => {
    // The `.ln` cells contain "10" / "11" / "12" — the helper must
    // not pick those, since gutters aren't user-selectable in the
    // production diff.
    const t = findFirstSelectableText(table);
    expect(t!.data).not.toBe('10');
  });

  test('returns null when there are no content pres', () => {
    document.body.innerHTML = '<table><tbody><tr><td class="ln">x</td></tr></tbody></table>';
    expect(findFirstSelectableText(document.querySelector('table')!)).toBeNull();
  });
});

describe('findLastSelectableText', () => {
  test('returns the last text node inside the last content pre', () => {
    const t = findLastSelectableText(table);
    expect(t).not.toBeNull();
    expect(t!.data).toBe('last line');
  });

  test('handles content cells nested inside syntax-highlight spans', () => {
    // The renderer wraps highlighted tokens in <span>s — the helper
    // walks down through them to reach the actual text nodes.
    document.body.innerHTML = `<table><tbody><tr><td class="content"><pre><span>function</span> <span>foo</span><span>() {}</span></pre></td></tr></tbody></table>`;
    const t = findLastSelectableText(document.querySelector('table')!);
    expect(t!.data).toBe('() {}');
  });

  test('returns null when there are no content pres', () => {
    document.body.innerHTML = '<table><tbody><tr><td class="ln">x</td></tr></tbody></table>';
    expect(findLastSelectableText(document.querySelector('table')!)).toBeNull();
  });
});
