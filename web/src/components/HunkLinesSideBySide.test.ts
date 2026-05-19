//! Component tests for `HunkLinesSideBySide`. Mirrors the unified-
//! mode coverage in `HunkLines.test.ts` (showComments gating, the
//! `.commented` row tag) and adds tests for the SBS-specific
//! affordances: the draggable divider that splits the two halves,
//! and the per-side rendering of the two columns.

import { fireEvent, render } from '@testing-library/svelte';
import type { ComponentProps } from 'svelte';
import { SvelteMap } from 'svelte/reactivity';
import { describe, expect, test, vi } from 'vitest';
import HunkLinesSideBySide from './HunkLinesSideBySide.svelte';
import type { CommentView, Hunk } from '../lib/types';

function hunk(): Hunk {
  return {
    base_range: { start: 1, end: 3 },
    tip_range: { start: 1, end: 3 },
    lines: [
      { origin: 'context', base_line: 1, tip_line: 1, content: 'one\n' },
      { origin: 'removed', base_line: 2, content: 'TWO\n' },
      { origin: 'added', tip_line: 2, content: 'Two\n' },
      { origin: 'context', base_line: 3, tip_line: 3, content: 'three\n' },
    ],
  };
}

function comment(over: Partial<CommentView> = {}): CommentView {
  return {
    schema_version: 1,
    comment_id: 'c1',
    session_id: 's1',
    review_id: 'r1',
    author: 'reviewer@example.com',
    created_at: '2026-05-15T10:00:00Z',
    patchset: 1,
    anchor_change_id: 'ch1',
    anchor_commit_id: 'co1',
    file: 'a.txt',
    side: 'tip',
    lines: { start: 2, end: 2 },
    flag: 'must-do',
    body: 'Nit.',
    anchor: { kind: 'valid' },
    draft: false,
    ...over,
  };
}

const noop = () => Promise.resolve();
const noopSync = () => {};

function renderSbs(
  props: Partial<ComponentProps<typeof HunkLinesSideBySide>> = {},
) {
  return render(HunkLinesSideBySide, {
    props: {
      hunk: hunk(),
      filePath: 'a.txt',
      comments: [],
      responses: [],
      currentPatchset: 1,
      composing: null,
      saving: false,
      highlights: {
        base: new SvelteMap<number, string>(),
        tip: new SvelteMap<number, string>(),
      },
      onstartcompose: noopSync,
      onreply: noop,
      onstatus: noop,
      ondelete: noop,
      onedit: noopSync,
      onselectpatchset: noopSync,
      ...props,
    },
  });
}

describe('HunkLinesSideBySide', () => {
  test('pairs removed/added lines into one row per side', () => {
    // The hunk has 4 lines (context, removed, added, context). The
    // pairing collapses the removed/added pair into a single
    // "change" row, giving 3 rows per side.
    const { container } = renderSbs();
    const halves = container.querySelectorAll('table.hunk-half');
    expect(halves.length).toBe(2);
    for (const half of halves) {
      expect(half.querySelectorAll('tr.sbs-row').length).toBe(3);
    }
  });

  test('renders the divider between the two sides', () => {
    const { container } = renderSbs();
    expect(container.querySelector('.sbs-divider')).not.toBeNull();
    // The hit-area overlay sits inside the divider.
    expect(container.querySelector('.sbs-divider-handle')).not.toBeNull();
  });

  test('applies the shared `sbsSplit` ratio to the base side', () => {
    const { container } = renderSbs({ sbsSplit: 0.7 });
    const base = container.querySelector('.sbs-side.base') as HTMLElement;
    expect(base.style.flexBasis).toContain('70%');
    const tip = container.querySelector('.sbs-side.tip') as HTMLElement;
    expect(tip.style.flexBasis).toContain('30%');
  });

  test('double-click on the divider resets the split to 0.5', async () => {
    const setSbsSplit = vi.fn();
    const { container } = renderSbs({ sbsSplit: 0.7, setSbsSplit });
    const divider = container.querySelector('.sbs-divider')!;
    await fireEvent.dblClick(divider);
    expect(setSbsSplit).toHaveBeenCalledWith(0.5);
  });

  test('drops the gutter +comment buttons in diffs-only mode', () => {
    const { container } = renderSbs({ showComments: false });
    expect(container.querySelectorAll('button.add-comment').length).toBe(0);
  });

  test('renders the inline thread row for an anchored comment', () => {
    const c = comment({ side: 'tip', lines: { start: 2, end: 2 } });
    const { container } = renderSbs({ comments: [c] });
    // The SBS template marks thread rows with `.sbs-threads`.
    expect(container.querySelector('tr.sbs-threads')).not.toBeNull();
  });

  test('hides the inline thread row in diffs-only mode', () => {
    const c = comment({ side: 'tip', lines: { start: 2, end: 2 } });
    const { container } = renderSbs({ comments: [c], showComments: false });
    expect(container.querySelector('tr.sbs-threads')).toBeNull();
  });
});
