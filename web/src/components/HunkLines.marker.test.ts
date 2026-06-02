//! Covers the uniform gutter-marker idiom (§12.4): every line that
//! anchors a comment or note carries a `>` fold chevron — not just
//! the outdated ones. Before this, a plain (valid-anchor) comment had
//! no marker and could only be folded by clicking its inline
//! highlight; the marker was reserved for outdated anchors. The test
//! asserts the chevron now renders for a valid single-anchor comment
//! and that clicking it toggles the thread, in lock-step with the
//! click-on-highlight path.

import { render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, test } from 'vitest';
import Host from './HunkLines.foldRefresh.test.svelte';
import type { CommentView, RegularHunk } from '../lib/types';

function hunk(): RegularHunk {
  return {
    kind: 'regular',
    base_range: { start: 1, end: 3 },
    tip_range: { start: 1, end: 3 },
    lines: [
      { origin: 'context', base_line: 1, tip_line: 1, content: 'one\n' },
      { origin: 'context', base_line: 2, tip_line: 2, content: 'two\n' },
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

describe('HunkLines gutter marker', () => {
  test('a valid (non-outdated) single comment renders a fold chevron', async () => {
    const { container } = render(Host, {
      props: { hunk: hunk(), comments: [comment()] },
    });
    await tick();

    // The marker is the uniform idiom — it must exist even though the
    // anchor is valid (the case that previously relied on the inline
    // highlight alone).
    expect(container.querySelector('.thread-marker')).not.toBeNull();
  });

  test('clicking the marker toggles the thread row', async () => {
    const { container } = render(Host, {
      props: { hunk: hunk(), comments: [comment()] },
    });
    await tick();

    // Open by default → row visible, marker expanded (not folded).
    expect(container.querySelector('tr.thread-row')).not.toBeNull();
    const marker = container.querySelector('.thread-marker') as HTMLElement;
    expect(marker.classList.contains('folded')).toBe(false);

    // Click folds: row disappears, marker flips to folded.
    marker.click();
    await tick();
    expect(container.querySelector('tr.thread-row')).toBeNull();
    expect(
      (container.querySelector('.thread-marker') as HTMLElement).classList
        .contains('folded'),
    ).toBe(true);

    // Click again expands: row returns.
    (container.querySelector('.thread-marker') as HTMLElement).click();
    await tick();
    expect(container.querySelector('tr.thread-row')).not.toBeNull();
  });
});
