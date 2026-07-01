import { describe, expect, it } from 'vitest';
import { shortenRevset } from './revset';

describe('shortenRevset', () => {
  it('short-SHAs a GitHub-imported `<base>..<head>` revset', () => {
    const rev =
      '0123456789abcdef0123456789abcdef01234567' +
      '..' +
      'fedcba9876543210fedcba9876543210fedcba98';
    expect(shortenRevset(rev)).toBe('012345678..fedcba987');
  });

  it('leaves symbolic revsets untouched', () => {
    expect(shortenRevset('main..feature/x')).toBe('main..feature/x');
    expect(shortenRevset('trunk()..@')).toBe('trunk()..@');
  });

  it('does not shorten runs shorter than a full SHA', () => {
    // Common short-SHA is 7–9 chars; those already look fine.
    expect(shortenRevset('abc1234..def5678')).toBe('abc1234..def5678');
  });

  it('is case-insensitive but preserves the original case', () => {
    const upper = 'ABCDEF0123456789ABCDEF0123456789ABCDEF01';
    expect(shortenRevset(`${upper}..HEAD`)).toBe('ABCDEF012..HEAD');
  });

  it('leaves 41-char hex runs alone (only exact 40-char SHAs shorten)', () => {
    // Guards against a regex that greedily matched any 40+ hex run
    // and truncated too aggressively.
    expect(shortenRevset('a'.repeat(41))).toBe('a'.repeat(41));
    expect(shortenRevset('a'.repeat(39))).toBe('a'.repeat(39));
  });

  it('handles multiple SHAs in one revset', () => {
    const a = '0'.repeat(40);
    const b = '1'.repeat(40);
    const c = '2'.repeat(40);
    expect(shortenRevset(`${a}..${b}|${c}`)).toBe('000000000..111111111|222222222');
  });
});
