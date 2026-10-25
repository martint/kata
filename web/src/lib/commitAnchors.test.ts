import { describe, expect, test } from 'vitest';
import { threadsInCommit, type Placeable } from './commitAnchors';
import type { AnchorView, CommitDiffView } from './types';

interface Thread extends Placeable {
  id: string;
}

const diff: CommitDiffView = {
  base_change: 'base-change',
  base_commit: 'base-commit',
  tip_change: 'tip-change',
  tip_commit: 'tip-commit',
  files: [],
};

function thread(id: string, fields: Partial<Thread>): Thread {
  return {
    id,
    anchor_commit_id: 'other-commit',
    side: 'tip',
    anchor: { kind: 'moved', new_lines: { start: 9, end: 9 } },
    ...fields,
  };
}

const idOf = (t: Thread) => t.id;

describe('threadsInCommit', () => {
  test('uses the anchor resolved against the commit, outdated ones too', () => {
    const moved = thread('m', {});
    const outdated = thread('o', {});
    const resolved: Record<string, AnchorView> = {
      m: { kind: 'moved', new_lines: { start: 2, end: 2 } },
      o: { kind: 'outdated', original_content: 'gone' },
    };
    expect(threadsInCommit([moved, outdated], idOf, resolved, diff)).toEqual([
      { ...moved, anchor: resolved.m },
      { ...outdated, anchor: resolved.o },
    ]);
  });

  test('keeps threads written on this commit at their stored lines', () => {
    // Neither is in `resolved`, like a draft written after the diff loaded.
    const tip = thread('t', { anchor_commit_id: 'tip-commit', side: 'tip' });
    const base = thread('b', { anchor_commit_id: 'base-commit', side: 'base' });
    expect(threadsInCommit([tip, base], idOf, undefined, diff)).toEqual([
      { ...tip, anchor: { kind: 'valid' } },
      { ...base, anchor: { kind: 'valid' } },
    ]);
  });

  test('leaves out a thread from another commit that has no resolved anchor', () => {
    const late = thread('l', {});
    // Written on the base commit but sitting on the tip side.
    const wrongSide = thread('w', { anchor_commit_id: 'base-commit', side: 'tip' });
    expect(threadsInCommit([late, wrongSide], idOf, {}, diff)).toEqual([]);
  });
});
