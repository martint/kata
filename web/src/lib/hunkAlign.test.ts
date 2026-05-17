//! Unit tests for the alignment algorithm. Covers the DP for various
//! block-shape cases plus the row-layout merge that consumes the
//! alignment.

import { describe, expect, test } from 'vitest';
import { alignBlock, alignedRows, lineSimilarity } from './hunkAlign';

describe('lineSimilarity', () => {
  test('identical strings score 1', () => {
    expect(lineSimilarity('foo bar', 'foo bar')).toBe(1);
  });

  test('empty strings score 0', () => {
    expect(lineSimilarity('', 'foo')).toBe(0);
    expect(lineSimilarity('foo', '')).toBe(0);
  });

  test('disjoint strings score 0', () => {
    expect(lineSimilarity('foo', 'bar')).toBe(0);
  });

  test('partial overlap scores between 0 and 1', () => {
    const s = lineSimilarity('the quick brown fox', 'the slow brown fox');
    expect(s).toBeGreaterThan(0.5);
    expect(s).toBeLessThan(1);
  });

  test('short line sharing one token with longer one scores low', () => {
    // The pairing DP must not be tricked into preferring a brand-new
    // short line over a long heavily-edited line, just because the
    // short one shares (proportionally) more tokens. `LCS/longer`
    // keeps this honest — `LCS/shorter` would score 1.0 here.
    const s = lineSimilarity(
      'function bar(a, b, c) { return a + b + c; }',
      'function newfunc() {',
    );
    expect(s).toBeLessThan(0.5);
  });

  test('two long lines sharing most tokens score high', () => {
    const s = lineSimilarity(
      'function bar(a, b, c) { return a + b + c; }',
      'function bar(a, b, c) { return a * b + c; }',
    );
    expect(s).toBeGreaterThan(0.8);
  });
});

describe('alignBlock', () => {
  test('pairs N:N blocks straight through when each pair is the best match', () => {
    const removes = ['line one', 'line two', 'line three'];
    const adds = ['line one updated', 'line two updated', 'line three updated'];
    const a = alignBlock(removes, adds);
    expect(a.pairs).toEqual([
      { removeIndex: 0, addIndex: 0 },
      { removeIndex: 1, addIndex: 1 },
      { removeIndex: 2, addIndex: 2 },
    ]);
    expect(a.unpairedRemoves).toEqual([]);
    expect(a.unpairedAdds).toEqual([]);
  });

  test('skips an inserted add line that has no match', () => {
    // r0↔a0, r1↔a2, a1 is a brand-new line.
    const removes = ['foo bar', 'baz qux'];
    const adds = ['foo bar updated', 'totally unrelated', 'baz qux updated'];
    const a = alignBlock(removes, adds);
    expect(a.pairs).toContainEqual({ removeIndex: 0, addIndex: 0 });
    expect(a.pairs).toContainEqual({ removeIndex: 1, addIndex: 2 });
    expect(a.unpairedRemoves).toEqual([]);
    expect(a.unpairedAdds).toEqual([1]);
  });

  test('skips a removed line that has no match', () => {
    // r0↔a0, r2↔a1, r1 is dropped entirely.
    const removes = ['foo bar', 'unrelated dropped', 'baz qux'];
    const adds = ['foo bar updated', 'baz qux updated'];
    const a = alignBlock(removes, adds);
    expect(a.pairs).toContainEqual({ removeIndex: 0, addIndex: 0 });
    expect(a.pairs).toContainEqual({ removeIndex: 2, addIndex: 1 });
    expect(a.unpairedRemoves).toEqual([1]);
    expect(a.unpairedAdds).toEqual([]);
  });

  test('refuses to pair lines below the similarity threshold', () => {
    // Block of pure-disjoint removes and adds — no token overlap
    // at all. The DP should leave everything unpaired rather than
    // forcing nonsense matches.
    const removes = ['aaa', 'bbb'];
    const adds = ['xxx', 'yyy'];
    const a = alignBlock(removes, adds);
    expect(a.pairs).toEqual([]);
    expect(a.unpairedRemoves).toEqual([0, 1]);
    expect(a.unpairedAdds).toEqual([0, 1]);
  });

  test('handles a pure-add block (no removes)', () => {
    const a = alignBlock([], ['one', 'two']);
    expect(a.pairs).toEqual([]);
    expect(a.unpairedRemoves).toEqual([]);
    expect(a.unpairedAdds).toEqual([0, 1]);
  });

  test('handles a pure-remove block (no adds)', () => {
    const a = alignBlock(['one', 'two'], []);
    expect(a.pairs).toEqual([]);
    expect(a.unpairedRemoves).toEqual([0, 1]);
    expect(a.unpairedAdds).toEqual([]);
  });
});

describe('alignedRows', () => {
  test('emits one row per pair when sizes match', () => {
    const a = alignBlock(['a one', 'b two'], ['a one!', 'b two!']);
    const rows = alignedRows(a);
    expect(rows).toEqual([
      { removeIndex: 0, addIndex: 0 },
      { removeIndex: 1, addIndex: 1 },
    ]);
  });

  test('interleaves an inserted add line on its own row', () => {
    const a = alignBlock(
      ['foo bar', 'baz qux'],
      ['foo bar updated', 'totally unrelated', 'baz qux updated'],
    );
    const rows = alignedRows(a);
    // r0 pairs with a0, then a1 sits alone (blank left), then r1
    // pairs with a2.
    expect(rows).toEqual([
      { removeIndex: 0, addIndex: 0 },
      { removeIndex: null, addIndex: 1 },
      { removeIndex: 1, addIndex: 2 },
    ]);
  });

  test('interleaves a dropped remove line on its own row', () => {
    const a = alignBlock(
      ['foo bar', 'dropped unrelated', 'baz qux'],
      ['foo bar updated', 'baz qux updated'],
    );
    const rows = alignedRows(a);
    expect(rows).toEqual([
      { removeIndex: 0, addIndex: 0 },
      { removeIndex: 1, addIndex: null },
      { removeIndex: 2, addIndex: 1 },
    ]);
  });

  test('fully-unpaired block falls back to index-zipping', () => {
    // No similarity passes the threshold, so the DP makes no pairs.
    // The layout shouldn't dump all adds before all removes — that
    // reads as "everything's gone, oh now everything's new." Fall
    // back to index-zipping (the pre-alignment behaviour) so the
    // shape stays compact and familiar; the absence of word-diff
    // highlights inside the rows is the signal that the rows aren't
    // actually paired in content.
    const a = alignBlock(['aaa', 'bbb'], ['xxx', 'yyy']);
    const rows = alignedRows(a);
    expect(rows).toEqual([
      { removeIndex: 0, addIndex: 0 },
      { removeIndex: 1, addIndex: 1 },
    ]);
  });

  test('single remove + short-new + paired-long: picks the long pair, not the short one', () => {
    // The exact failure mode reported on a real diff. Diff structure:
    //   - L2 (long line that maps to R2)
    //   + NC (short new line, shares one token with L2)
    //   + R2 (heavily edited continuation of L2)
    // Naive LCS-over-shorter would prefer L2↔NC (the short line
    // scores artificially high). LCS-over-longer correctly prefers
    // L2↔R2 because that pair shares far more of L2's content.
    const a = alignBlock(
      ['function bar(a, b, c) { return a + b + c; }'],
      [
        'function newfunc() {',
        'function bar(a, b, c) { return a * b + c; }',
      ],
    );
    const rows = alignedRows(a);
    expect(rows).toEqual([
      { removeIndex: null, addIndex: 0 },
      { removeIndex: 0, addIndex: 1 },
    ]);
  });

  test('user case A: identical lines surrounding an insertion pair correctly', () => {
    // Diff structure:
    //   - L2 (removed)
    //   + NC (added, new content)
    //   + L2 (re-added — identical to the removed one)
    // Should align as:
    //         NC   (no left, NC right)
    //   L2    L2   (paired)
    const a = alignBlock(
      ['function foo() { do_thing(); }'],
      [
        'function newfunc() {',
        'function foo() { do_thing(); }',
      ],
    );
    const rows = alignedRows(a);
    expect(rows).toEqual([
      { removeIndex: null, addIndex: 0 },
      { removeIndex: 0, addIndex: 1 },
    ]);
  });

  test('user case B: new line on top, then two paired pairs', () => {
    // Diff structure:
    //   - L1 (removed)
    //   - L2 (removed)
    //   + NC (added, new content at the top)
    //   + L1' (added, edited L1)
    //   + L2' (added, edited L2)
    // Should align as:
    //         NC    (no left, NC right)
    //   L1    L1'   (paired)
    //   L2    L2'   (paired)
    const a = alignBlock(
      [
        'function alpha(x) { return x; }',
        'function beta(y) { return y; }',
      ],
      [
        'function brandnew() {',
        'function alpha(x) { return x + 1; }',
        'function beta(y) { return y + 2; }',
      ],
    );
    const rows = alignedRows(a);
    expect(rows).toEqual([
      { removeIndex: null, addIndex: 0 },
      { removeIndex: 0, addIndex: 1 },
      { removeIndex: 1, addIndex: 2 },
    ]);
  });

  test('user-reported: r0↔a0 + new line a1 + r1↔a2 lines up correctly', () => {
    // The bug case the user hit. Strict-index pairing produced:
    //   L1   R1
    //   L2   NC      ← spurious pair, L2 and NC aren't related
    //        R2     ← R2 stranded
    // Aligned should produce:
    //   L1   R1
    //        NC
    //   L2   R2
    const a = alignBlock(
      ['function foo() {', 'function bar() {'],
      ['function foo() {', 'function baz() {', 'function bar() {'],
    );
    const rows = alignedRows(a);
    expect(rows).toEqual([
      { removeIndex: 0, addIndex: 0 },
      { removeIndex: null, addIndex: 1 },
      { removeIndex: 1, addIndex: 2 },
    ]);
  });

  test('fully-unpaired uneven block index-zips with blanks on the shorter side', () => {
    const a = alignBlock(['aaa'], ['xxx', 'yyy', 'zzz']);
    const rows = alignedRows(a);
    expect(rows).toEqual([
      { removeIndex: 0, addIndex: 0 },
      { removeIndex: null, addIndex: 1 },
      { removeIndex: null, addIndex: 2 },
    ]);
  });
});
