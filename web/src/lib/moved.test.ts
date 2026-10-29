import { describe, expect, test } from 'vitest';
import {
  movedBlockOf,
  movedBlockStartingAt,
  movedClass,
  movedHref,
  movedLabel,
  movedLookup,
  movedTitle,
  moveKey,
} from './moved';
import type { MovedLines } from './types';

const away: MovedLines = {
  side: 'base',
  lines: { start: 3, end: 6 },
  other: { path: 'shop/pricing/discounts.py', lines: { start: 1, end: 4 } },
};
const here: MovedLines = {
  side: 'tip',
  lines: { start: 9, end: 9 },
  alt: true,
  other: { path: 'billing.py', lines: { start: 2, end: 2 } },
};

describe('moved lines', () => {
  const lookup = movedLookup([away, here]);

  test('finds the block by side and line', () => {
    expect(lookup('base', 3)).toBe(away);
    expect(lookup('base', 6)).toBe(away);
    expect(lookup('base', 7)).toBeNull();
    expect(lookup('tip', 3)).toBeNull();
    expect(lookup('tip', null)).toBeNull();
    expect(movedLookup(undefined)('base', 3)).toBeNull();
  });

  test('maps hunk lines to their side', () => {
    expect(movedBlockOf(lookup, { origin: 'removed', base_line: 4, content: 'x\n' })).toBe(away);
    expect(movedBlockOf(lookup, { origin: 'added', tip_line: 9, content: 'x\n' })).toBe(here);
    expect(movedBlockOf(lookup, { origin: 'context', base_line: 4, tip_line: 4, content: 'x\n' })).toBeNull();
  });

  test('reports a block only at its first line', () => {
    expect(movedBlockStartingAt(lookup, { origin: 'removed', base_line: 3, content: '' })).toBe(away);
    expect(movedBlockStartingAt(lookup, { origin: 'removed', base_line: 4, content: '' })).toBeNull();
    expect(movedBlockStartingAt(lookup, null)).toBeNull();
  });

  test('labels the other end by file name and drops it for a move within the file', () => {
    expect(movedLabel(away, 'billing.py')).toBe('Code moved to discounts.py lines 1–4');
    expect(movedLabel(here, 'billing.py')).toBe('Code moved from line 2');
  });

  test('titles the other end with its full path', () => {
    expect(movedTitle(away, 'billing.py')).toBe('Code moved to shop/pricing/discounts.py lines 1–4');
  });

  test('links to the other end on the opposite side', () => {
    expect(movedHref(away)).toBe('#L:shop%2Fpricing%2Fdiscounts.py:tip:1-4');
    expect(movedHref(here)).toBe('#L:billing.py:base:2');
  });

  test('keys both ends of a block the same', () => {
    const otherEnd: MovedLines = {
      side: 'tip',
      lines: { start: 1, end: 4 },
      other: { path: 'billing.py', lines: { start: 3, end: 6 } },
    };
    expect(moveKey(away, 'billing.py')).toBe('billing.py:3>shop/pricing/discounts.py:1');
    expect(moveKey(otherEnd, 'shop/pricing/discounts.py')).toBe(moveKey(away, 'billing.py'));
  });

  test('classes name the end and the zebra colour', () => {
    expect(movedClass(away)).toBe(' moved moved-from');
    expect(movedClass(here)).toBe(' moved moved-to moved-alt');
    expect(movedClass(null)).toBe('');
  });
});
