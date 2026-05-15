import { describe, expect, test } from 'vitest';

import { computeHunkWordDiff, diffLines, wrapRanges } from './wordDiff';

describe('diffLines', () => {
  test('returns null for empty inputs', () => {
    expect(diffLines('', 'foo')).toBeNull();
    expect(diffLines('foo', '')).toBeNull();
  });

  test('returns empty ranges for identical inputs', () => {
    expect(diffLines('foo', 'foo')).toEqual({ removed: [], added: [] });
  });

  test('detects a renamed identifier', () => {
    // "let foo = 1" -> "let bar = 1": only foo/bar differ.
    const d = diffLines('let foo = 1', 'let bar = 1');
    expect(d).not.toBeNull();
    expect(d!.removed).toEqual([{ start: 4, end: 7 }]);
    expect(d!.added).toEqual([{ start: 4, end: 7 }]);
  });

  test('still produces a diff for low-overlap pairs', () => {
    // The pair was matched at the line level, so we keep showing
    // word-diff even for very different content — the alternative
    // (returning null) would silently swallow the highlighting on
    // exactly the cases where it most needs to confirm what changed.
    const d = diffLines('apple banana cherry', 'foxtrot golf hotel');
    expect(d).not.toBeNull();
    expect(d!.removed.length).toBeGreaterThan(0);
    expect(d!.added.length).toBeGreaterThan(0);
  });

  test('handles added text in the middle', () => {
    const d = diffLines('foo(x)', 'foo(x, y)');
    expect(d).not.toBeNull();
    // Removed side has no extra characters; added side has ", y" tokens.
    expect(d!.removed).toEqual([]);
    expect(d!.added.length).toBeGreaterThan(0);
  });

  test('handles deleted text in the middle', () => {
    const d = diffLines('foo(x, y)', 'foo(x)');
    expect(d).not.toBeNull();
    expect(d!.added).toEqual([]);
    expect(d!.removed.length).toBeGreaterThan(0);
  });
});

describe('computeHunkWordDiff', () => {
  test('skips unbalanced blocks', () => {
    const lines = [
      { origin: 'removed' as const, content: 'foo\n' },
      { origin: 'removed' as const, content: 'bar\n' },
      { origin: 'added' as const, content: 'baz\n' },
    ];
    expect(computeHunkWordDiff(lines).size).toBe(0);
  });

  test('pairs N:N blocks row by row', () => {
    const lines = [
      { origin: 'context' as const, content: 'context\n' },
      { origin: 'removed' as const, content: 'let foo = 1\n' },
      { origin: 'removed' as const, content: 'let qux = 2\n' },
      { origin: 'added' as const, content: 'let bar = 1\n' },
      { origin: 'added' as const, content: 'let zap = 2\n' },
    ];
    const out = computeHunkWordDiff(lines);
    // Both remove rows AND both add rows get annotations.
    expect(out.size).toBe(4);
    expect(out.has(1)).toBe(true); // first removed
    expect(out.has(2)).toBe(true); // second removed
    expect(out.has(3)).toBe(true); // first added
    expect(out.has(4)).toBe(true); // second added
    expect(out.get(1)!.kind).toBe('removed');
    expect(out.get(3)!.kind).toBe('added');
  });

  test('returns empty map for pure context', () => {
    const lines = [
      { origin: 'context' as const, content: 'a\n' },
      { origin: 'context' as const, content: 'b\n' },
    ];
    expect(computeHunkWordDiff(lines).size).toBe(0);
  });
});

describe('wrapRanges', () => {
  test('wraps a range inside a single span', () => {
    const html = '<span style="color:red">hello world</span>';
    const out = wrapRanges(html, [{ start: 6, end: 11 }], 'added');
    expect(out).toContain('<span class="wd-added">world</span>');
  });

  test('wraps across span boundaries', () => {
    // Two adjacent shiki spans; the range crosses the boundary.
    const html =
      '<span style="color:red">let foo</span><span style="color:blue"> = 1</span>';
    const out = wrapRanges(html, [{ start: 4, end: 9 }], 'removed');
    // "foo = " (positions 4-9) should be wrapped — split across both spans.
    // Verify a wd-removed span exists and the original color styles survive.
    expect(out).toMatch(/wd-removed/);
    expect(out).toContain('color:red');
    expect(out).toContain('color:blue');
  });

  test('passes through unchanged when ranges is empty', () => {
    const html = '<span style="color:red">hello</span>';
    expect(wrapRanges(html, [], 'added')).toBe(html);
  });

  test('escapes special characters when reserializing text', () => {
    const html = '<span style="color:red">a &lt; b</span>';
    const out = wrapRanges(html, [{ start: 0, end: 1 }], 'added');
    // The `<` we exposed was an entity in the source; if the wrapper
    // decodes-and-reescapes it should come back as `&lt;`, not `<`.
    expect(out).toContain('&lt;');
    expect(out).not.toMatch(/[^&]<\s/);
  });
});
