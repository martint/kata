//! Regression test for: comment folding stops working after an
//! SSE-driven `refresh()` replaces `current.comments` with a fresh
//! server response. The fold store + version-counter contexts live
//! at the ReviewViewer level (so they survive the refresh) but the
//! per-line aggregate `allFoldedAt` in HunkLines reads through both —
//! a regression in either the context wiring or the way HunkLines
//! tracks the comments prop would silently break clicks on the
//! gutter marker and on highlighted commented text.

import { render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, test } from 'vitest';
import Host from './HunkLines.foldRefresh.test.svelte';
import type {
  CommentView,
  RegularHunk,
  ResponseView,
} from '../lib/types';

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

describe('HunkLines fold after refresh', () => {
  test(
    'clicking highlighted commented text continues to toggle the thread row ' +
      'after the comments prop is replaced (simulating SSE refresh)',
    async () => {
      const initial = [comment()];
      const { container, rerender } = render(Host, {
        props: { hunk: hunk(), comments: initial },
      });
      await tick();

      // Thread row should be visible (default fold state is expanded
      // for an open thread in non-Compact mode).
      expect(container.querySelector('tr.thread-row')).not.toBeNull();

      // Simulate the SSE-driven refresh: replace `comments` with a
      // *new array of fresh objects* whose comment_id matches the
      // original. This is what ReviewViewer does when `current`
      // gets reassigned with a freshly-deserialised server response.
      await rerender({ hunk: hunk(), comments: [comment()] });
      await tick();

      // After refresh the row is still visible (same default).
      expect(container.querySelector('tr.thread-row')).not.toBeNull();

      // Click on the highlighted commented text on the anchor row.
      // The handler is `onContentClick`, which checks for a click on
      // `.column-anchor` OR a `.commented-fullline` row. In our
      // fixture (single-line, no column range), the row is full-line
      // commented — click anywhere on `.content` toggles fold.
      const anchorRow = container.querySelector(
        'tr.row.commented-fullline',
      ) as HTMLElement | null;
      expect(anchorRow).not.toBeNull();
      const contentCell = anchorRow!.querySelector(
        'td.content',
      ) as HTMLElement | null;
      expect(contentCell).not.toBeNull();
      contentCell!.click();
      await tick();

      // The thread should now be folded — the inline thread-row
      // disappears (`!allFoldedAt(a)` flips false). If `foldStore.set`
      // fires but the consumers don't re-derive, this assertion will
      // fail.
      expect(container.querySelector('tr.thread-row')).toBeNull();
    },
  );

  test(
    'clicking a thread that is force-expanded by unread replies actually ' +
      'folds it (regression: in-memory lastVisitAt stale after SSE refresh)',
    async () => {
      // Reproduces the bug: after an SSE-driven refresh, the
      // in-memory `lastVisitAt` lags behind any replies that arrived
      // since the page was first loaded. A thread the user had
      // previously folded (foldStore.get → true) is force-expanded
      // by `hasUnreadReplies(thread, responses, lastVisitAt, viewer)`
      // returning true. The click handler then computes
      // `target = anyExpanded = true` (folding "everything"), but
      // because the stored fold flag was already `true` the
      // `foldStore.set` is a silent no-op — no flush, no re-render
      // input changes, the unread force keeps the thread visible.
      //
      // Fix: every fold click also adds the affected ids to the
      // session-local `acknowledgedUnread` context; `hasUnreadReplies`
      // returns false for acknowledged ids. The next render sees no
      // unread force, evaluates the stored fold honestly, and hides
      // the thread.
      const c = comment();
      const reply: ResponseView = {
        schema_version: 1,
        response_id: 'r1',
        session_id: 's2',
        author: 'someone-else@example.com',
        in_reply_to: 'c1',
        action: 'comment',
        body: 'reply',
        created_at: '2026-05-20T12:00:00Z',
        draft: false,
      };
      // lastVisitAt earlier than the reply — exactly the post-SSE-
      // refresh shape where in-memory lags reality.
      const lastVisitAt = '2026-05-19T00:00:00Z';
      const { container } = render(Host, {
        props: {
          hunk: hunk(),
          comments: [c],
          responses: [reply],
          lastVisitAt,
          viewer: 'reviewer@example.com',
          // Pre-fold the thread the way a prior session would have.
          seedFolds: { c1: true },
        },
      });
      await tick();

      // Thread renders despite the stored fold, because unread-force
      // overrides it. This is the buggy starting state.
      expect(container.querySelector('tr.thread-row')).not.toBeNull();

      // Click on the highlighted commented row to fold.
      const contentCell = container.querySelector(
        'tr.row.commented-fullline td.content',
      ) as HTMLElement | null;
      expect(contentCell).not.toBeNull();
      contentCell!.click();
      await tick();

      // After the click, the thread should be hidden. Without the
      // acknowledgement fix this assertion fails because the
      // foldStore write was a no-op and the unread-force kept the
      // row visible.
      expect(container.querySelector('tr.thread-row')).toBeNull();
    },
  );
});
