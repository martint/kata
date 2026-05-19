//! Component tests for `CommentThread`. Focused on the behaviour that
//! falls out of the prop shape — anchor labels, resolution badges,
//! the unread-replies signal, the edit-mode hide, the resolved-but-
//! unread auto-expand. The rendering is intentionally exercised
//! through `@testing-library/svelte` so a future refactor of the
//! template that breaks any of these stays caught.

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import type { ComponentProps } from 'svelte';
import { describe, expect, test } from 'vitest';
import CommentThread from './CommentThread.svelte';
import type { CommentView, ResponseView } from '../lib/types';

/** Build a published comment view with sensible defaults so each
 *  test names only the fields it cares about. */
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
    lines: { start: 1, end: 1 },
    flag: 'must-do',
    body: 'Please address.',
    anchor: { kind: 'valid' },
    draft: false,
    ...over,
  };
}

/** Build a published response view with sensible defaults. */
function response(over: Partial<ResponseView> = {}): ResponseView {
  return {
    schema_version: 1,
    response_id: 'r-1',
    in_reply_to: 'c1',
    session_id: 's2',
    author: 'author@example.com',
    created_at: '2026-05-15T12:00:00Z',
    action: 'comment',
    body: 'OK, done.',
    draft: false,
    ...over,
  };
}

const noop = () => Promise.resolve();
const noopSync = () => {};

function renderThread(props: Partial<ComponentProps<typeof CommentThread>> = {}) {
  return render(CommentThread, {
    props: {
      comments: [comment()],
      responses: [],
      saving: false,
      onreply: noop,
      onstatus: noop,
      ondelete: noop,
      onedit: noopSync,
      ...props,
    },
  });
}

describe('CommentThread', () => {
  test('renders an open comment with its author and body', () => {
    renderThread();
    expect(screen.getByText('reviewer@example.com')).toBeTruthy();
    expect(screen.getByText('Please address.')).toBeTruthy();
    // The default flag (`must-do`) is suppressed to keep the header
    // uncluttered — most comments are must-do, so the chip would
    // appear on every row.
    expect(screen.queryByText('must-do')).toBeNull();
  });

  test('shows the flag chip when it differs from the default', () => {
    renderThread({ comments: [comment({ flag: 'question' })] });
    expect(screen.getByText('question')).toBeTruthy();
  });

  test('omits the resolved-state badge while the comment is still open', () => {
    renderThread();
    expect(screen.queryByText('resolved')).toBeNull();
    expect(screen.queryByText("won't fix")).toBeNull();
  });

  test('folds a resolved comment by default; chevron click expands', async () => {
    // Resolution-aware default fold: a resolved thread defaults to
    // header-only inside CommentThread. The badge is suppressed in
    // the collapsed header (the fact that it's collapsed-by-default
    // IS the signal) and rendered when expanded. `showFold` is on
    // here because production-side groups of 1 hide the chevron in
    // favour of the gutter marker; standalone tests don't have a
    // marker so the chevron has to render to be testable.
    const c = comment({ comment_id: 'c1' });
    const r = response({
      response_id: 'r-1',
      in_reply_to: 'c1',
      action: 'resolve',
    });
    const { container } = renderThread({ comments: [c], responses: [r], showFold: true });
    expect(screen.queryByText('resolved')).toBeNull();
    const li = container.querySelector('.comment')!;
    expect(li.classList.contains('collapsed')).toBe(true);
    expect(li.querySelector('.body')).toBeNull();
    const fold = li.querySelector('button.fold-toggle') as HTMLButtonElement;
    await fireEvent.click(fold);
    expect(li.querySelector('.badge.resolution-resolved')).not.toBeNull();
  });

  test('flags a comment with new replies since the viewer last visited', () => {
    const c = comment({ comment_id: 'c1' });
    // Reply by someone other than the viewer, AFTER the last visit.
    const r = response({
      in_reply_to: 'c1',
      author: 'author@example.com',
      created_at: '2026-05-16T09:00:00Z',
    });
    const { container } = renderThread({
      comments: [c],
      responses: [r],
      lastVisitAt: '2026-05-15T20:00:00Z',
      viewer: 'reviewer@example.com',
    });
    expect(screen.getByText('new replies')).toBeTruthy();
    expect(container.querySelector('.comment')!.classList.contains('unread')).toBe(true);
  });

  test("doesn't flag replies the viewer wrote themselves as unread", () => {
    const c = comment({ comment_id: 'c1', author: 'reviewer@example.com' });
    const r = response({
      in_reply_to: 'c1',
      author: 'reviewer@example.com',
      created_at: '2026-05-16T09:00:00Z',
    });
    renderThread({
      comments: [c],
      responses: [r],
      lastVisitAt: '2026-05-15T20:00:00Z',
      viewer: 'reviewer@example.com',
    });
    expect(screen.queryByText('new replies')).toBeNull();
  });

  test("doesn't flag replies posted before the viewer's last visit", () => {
    const c = comment({ comment_id: 'c1' });
    const r = response({
      in_reply_to: 'c1',
      author: 'author@example.com',
      created_at: '2026-05-14T09:00:00Z',
    });
    renderThread({
      comments: [c],
      responses: [r],
      lastVisitAt: '2026-05-15T20:00:00Z',
      viewer: 'reviewer@example.com',
    });
    expect(screen.queryByText('new replies')).toBeNull();
  });

  test('keeps a resolved-but-unread thread expanded so its body stays visible', () => {
    // The whole point of the unread marker for the agent workflow:
    // when the AI marks every comment resolved, the user shouldn't
    // have to expand each one to read the response.
    const c = comment({ comment_id: 'c1' });
    const r = response({
      in_reply_to: 'c1',
      author: 'author@example.com',
      created_at: '2026-05-16T09:00:00Z',
      action: 'resolve',
    });
    const { container } = renderThread({
      comments: [c],
      responses: [r],
      lastVisitAt: '2026-05-15T20:00:00Z',
      viewer: 'reviewer@example.com',
    });
    const li = container.querySelector('.comment')!;
    expect(li.classList.contains('collapsed')).toBe(false);
    // Body should be present (not hidden under .collapsed).
    expect(li.querySelector('.body')).not.toBeNull();
  });

  test('hides the comment whose id matches editingCommentId', () => {
    const c1 = comment({ comment_id: 'c1', body: 'first' });
    const c2 = comment({ comment_id: 'c2', body: 'second' });
    renderThread({ comments: [c1, c2], editingCommentId: 'c2' });
    expect(screen.getByText('first')).toBeTruthy();
    expect(screen.queryByText('second')).toBeNull();
  });

  test('surfaces an anchor-drift label when the anchor has moved', () => {
    const c = comment({
      anchor: { kind: 'moved', new_lines: { start: 5, end: 7 } },
    });
    renderThread({ comments: [c] });
    expect(screen.getByText('moved to 5-7')).toBeTruthy();
  });

  test("folds a won't-fix-marked thread by default; chevron expands", async () => {
    // Mirrors the resolved-comment test: a wont-fix response also
    // makes the thread default-fold; badge appears on expand.
    const c = comment({ comment_id: 'c1' });
    const r = response({
      in_reply_to: 'c1',
      action: 'wont-fix',
      author: 'author@example.com',
      body: 'Not for this round.',
    });
    const { container } = renderThread({ comments: [c], responses: [r], showFold: true });
    const li = container.querySelector('.comment')!;
    expect(li.classList.contains('collapsed')).toBe(true);
    expect(li.querySelector('.badge.resolution-wont-fix')).toBeNull();
    const fold = li.querySelector('button.fold-toggle') as HTMLButtonElement;
    await fireEvent.click(fold);
    expect(li.querySelector('.badge.resolution-wont-fix')).not.toBeNull();
  });

  test('shows replies for an open comment with a comment-action response', () => {
    const c = comment({ comment_id: 'c1' });
    const r = response({
      in_reply_to: 'c1',
      action: 'comment',
      author: 'author@example.com',
      body: 'Acknowledged.',
    });
    const { container } = renderThread({ comments: [c], responses: [r] });
    const replies = container.querySelector('.replies');
    expect(replies).not.toBeNull();
    expect(within(replies as HTMLElement).getByText('Acknowledged.')).toBeTruthy();
    expect(within(replies as HTMLElement).getByText('replied')).toBeTruthy();
  });
});
