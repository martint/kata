//! Tests for `compareBaseCommit` — the chooser that decides which
//! file version the base side of a compare-mode diff highlights
//! against. The interesting case is the per-commit interdiff, where
//! the base is the selected pair's from-side commit and NOT the
//! compared patchset's tip (the regression that made removed-side
//! rows render text from unrelated lines).

import { describe, expect, test } from 'vitest';
import { compareBaseCommit, type PatchsetTip } from './compareBase';

const patchsets: PatchsetTip[] = [
  { n: 1, tip_commit: 'ps1-tip' },
  { n: 2, tip_commit: 'ps2-tip' },
  { n: 3, tip_commit: 'ps3-tip' },
];

describe('compareBaseCommit', () => {
  test('returns null outside compare mode', () => {
    expect(compareBaseCommit(null, null, patchsets)).toBeNull();
  });

  test('whole-patchset compare: the compared patchset tip', () => {
    expect(compareBaseCommit(null, 1, patchsets)).toBe('ps1-tip');
    expect(compareBaseCommit(null, 2, patchsets)).toBe('ps2-tip');
  });

  test('unknown compared patchset resolves to null', () => {
    expect(compareBaseCommit(null, 9, patchsets)).toBeNull();
  });

  test('per-commit interdiff: the pair from-commit wins over the compared patchset tip', () => {
    // A pair selected while comparing PS1→PS3: the diff base is the
    // pair's parent/from commit, not PS1's tip. Using the tip would
    // highlight the base column from the wrong file.
    expect(compareBaseCommit('pair-parent-commit', 1, patchsets)).toBe(
      'pair-parent-commit',
    );
  });

  test('per-commit interdiff takes precedence even with no compareWith', () => {
    expect(compareBaseCommit('pair-parent-commit', null, patchsets)).toBe(
      'pair-parent-commit',
    );
  });
});
