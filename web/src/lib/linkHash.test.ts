//! Tests for the line-range URL hash codec. The format is the
//! contract between the SelectionPopup's "Copy permalink" button
//! and ReviewViewer's `jumpToHash` — round-trip stability matters
//! because the user shares these URLs across tools.

import { describe, expect, test } from 'vitest';
import { lineRangeHash, parseLineRangeHash } from './linkHash';

describe('lineRangeHash', () => {
  test('renders a single-line range without the trailing -end', () => {
    expect(lineRangeHash({ file: 'src/foo.rs', side: 'tip', startLine: 42, endLine: 42 })).toBe(
      '#L:src%2Ffoo.rs:tip:42',
    );
  });

  test('renders a multi-line range with start-end', () => {
    expect(lineRangeHash({ file: 'src/foo.rs', side: 'tip', startLine: 42, endLine: 50 })).toBe(
      '#L:src%2Ffoo.rs:tip:42-50',
    );
  });

  test('URL-encodes paths that contain colons or spaces', () => {
    expect(
      lineRangeHash({ file: 'with:colon and space.rs', side: 'base', startLine: 1, endLine: 1 }),
    ).toBe('#L:with%3Acolon%20and%20space.rs:base:1');
  });
});

describe('parseLineRangeHash', () => {
  test('round-trips a single-line link', () => {
    const link = { file: 'src/foo.rs', side: 'tip' as const, startLine: 42, endLine: 42 };
    expect(parseLineRangeHash(lineRangeHash(link))).toEqual(link);
  });

  test('round-trips a multi-line link', () => {
    const link = { file: 'src/foo.rs', side: 'base' as const, startLine: 10, endLine: 25 };
    expect(parseLineRangeHash(lineRangeHash(link))).toEqual(link);
  });

  test('accepts a hash without the leading #', () => {
    expect(parseLineRangeHash('L:foo:tip:5')).toEqual({
      file: 'foo',
      side: 'tip',
      startLine: 5,
      endLine: 5,
    });
  });

  test('round-trips a path that contains a colon (decoded back to its literal form)', () => {
    const link = { file: 'odd:path.rs', side: 'tip' as const, startLine: 1, endLine: 2 };
    expect(parseLineRangeHash(lineRangeHash(link))).toEqual(link);
  });

  test('returns null for hashes that are not L: links', () => {
    expect(parseLineRangeHash('#c-abc')).toBeNull();
    expect(parseLineRangeHash('#file-foo.rs')).toBeNull();
    expect(parseLineRangeHash('#')).toBeNull();
    expect(parseLineRangeHash('')).toBeNull();
  });

  test('returns null when the side is not base or tip', () => {
    expect(parseLineRangeHash('#L:foo:side:1')).toBeNull();
  });

  test('returns null when lines are non-positive or end < start', () => {
    expect(parseLineRangeHash('#L:foo:tip:0')).toBeNull();
    expect(parseLineRangeHash('#L:foo:tip:-3')).toBeNull();
    expect(parseLineRangeHash('#L:foo:tip:10-5')).toBeNull();
  });

  test('returns null when the format is malformed (too few parts)', () => {
    expect(parseLineRangeHash('#L:foo')).toBeNull();
    expect(parseLineRangeHash('#L:foo:tip')).toBeNull();
  });
});
