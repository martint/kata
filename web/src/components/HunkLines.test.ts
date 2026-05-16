//! Component tests for `HunkLines` (the unified-diff renderer).
//! Verifies the `showComments` gating that the recent display-mode
//! work added, the per-row `.commented` stripe behaviour, and that
//! the gutter `+` button only appears on hover-eligible rows. Render
//! is real (via @testing-library/svelte) so the template wiring
//! itself stays under test.

import { render } from '@testing-library/svelte';
import { SvelteMap } from 'svelte/reactivity';
import { describe, expect, test } from 'vitest';
import HunkLines from './HunkLines.svelte';
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

function renderHunk(props: Partial<Parameters<typeof HunkLines>[0]> = {}) {
  return render(HunkLines as unknown as never, {
    props: {
      hunk: hunk(),
      filePath: 'a.txt',
      comments: [],
      responses: [],
      currentPatchset: 1,
      composing: null,
      saving: false,
      highlights: { base: new SvelteMap(), tip: new SvelteMap() },
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

describe('HunkLines', () => {
  test('renders one row per hunk line in order', () => {
    const { container } = renderHunk();
    const rows = container.querySelectorAll('tr.row');
    expect(rows.length).toBe(4);
    // Quick sanity: row classes carry the line origin.
    const classes = Array.from(rows).map((r) => r.className);
    expect(classes[0]).toContain('context');
    expect(classes[1]).toContain('removed');
    expect(classes[2]).toContain('added');
    expect(classes[3]).toContain('context');
  });

  test('renders the +comment gutter button by default', () => {
    const { container } = renderHunk();
    // Each row with a non-empty anchor (i.e. has a line number on the
    // shown side) gets a +comment button. With `lineNumberMode = 'both'`,
    // the button lives in the tip-side .ln cell.
    expect(container.querySelectorAll('button.add-comment').length).toBeGreaterThan(0);
  });

  test('drops the +comment buttons in diffs-only mode (showComments=false)', () => {
    const { container } = renderHunk({ showComments: false });
    expect(container.querySelectorAll('button.add-comment').length).toBe(0);
  });

  test('drops the inline thread row in diffs-only mode', () => {
    const c = comment({
      side: 'tip',
      lines: { start: 2, end: 2 },
    });
    const { container } = renderHunk({
      comments: [c],
      showComments: false,
    });
    expect(container.querySelector('tr.thread-row')).toBeNull();
  });

  test('renders the inline thread row when a comment anchors here', () => {
    const c = comment({
      side: 'tip',
      lines: { start: 2, end: 2 },
    });
    const { container } = renderHunk({ comments: [c] });
    expect(container.querySelector('tr.thread-row')).not.toBeNull();
  });

  test('tags the row with `.commented` when a comment anchors to that line', () => {
    const c = comment({
      side: 'tip',
      lines: { start: 2, end: 2 },
    });
    const { container } = renderHunk({ comments: [c] });
    // The CSS hook lives on `tr.row.commented` (and is read down to
    // the .content cell via the descendant selector in the
    // stylesheet); the unit assertion stays at the row level.
    expect(container.querySelectorAll('tr.row.commented').length).toBeGreaterThan(0);
  });

  test('does NOT tag rows as commented when showComments is off', () => {
    const c = comment({
      side: 'tip',
      lines: { start: 2, end: 2 },
    });
    const { container } = renderHunk({ comments: [c], showComments: false });
    expect(container.querySelectorAll('tr.row.commented').length).toBe(0);
  });
});
