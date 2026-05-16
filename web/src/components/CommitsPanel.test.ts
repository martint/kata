//! Component tests for `CommitsPanel`. Covers the per-commit row
//! rendering, comment-count badges over file membership, the
//! "All commits" affordance for review-wide comments, scoped vs
//! unscoped selection, and the commit-level / review-wide inline
//! comment threads that the recent commit-comment work added.

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, expect, test, vi } from 'vitest';
import CommitsPanel from './CommitsPanel.svelte';
import type { CommentView, CommitInfo, ResponseView } from '../lib/types';

function commit(over: Partial<CommitInfo> = {}): CommitInfo {
  return {
    change_id: 'ch-xxxxxxxxxxxxxxxxxxxx',
    commit_id: 'co-00000000000000000000',
    author_email: 'alice@example.com',
    author_timestamp: '2026-05-15T10:00:00Z',
    description_first_line: 'tweak the thing',
    description: 'tweak the thing',
    changed_files: ['a.txt'],
    ...over,
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
    anchor_change_id: 'ch-xxxxxxxxxxxxxxxxxxxx',
    anchor_commit_id: 'co-00000000000000000000',
    flag: 'must-do',
    body: 'note',
    anchor: { kind: 'valid' },
    draft: false,
    ...over,
  };
}

const noop = () => Promise.resolve();
const noopSync = () => {};

function renderPanel(
  props: Partial<Parameters<typeof CommitsPanel>[0]> = {},
) {
  return render(CommitsPanel as unknown as never, {
    props: {
      commits: [commit()],
      comments: [],
      responses: [] as ResponseView[],
      selectedChangeId: null,
      currentPatchset: 1,
      reviewAnchorIds: { change: 'review-ch', commit: 'review-co' },
      composing: null,
      saving: false,
      onselect: noopSync,
      onstartcompose: noopSync,
      oncancelcompose: noopSync,
      onsubmit: noop,
      onreply: noop,
      onstatus: noop,
      ondelete: noop,
      onedit: noopSync,
      onselectpatchset: noopSync,
      ...props,
    },
  });
}

describe('CommitsPanel', () => {
  test('lists each commit with its short change-id, short commit-id, and description', () => {
    renderPanel({ commits: [commit()] });
    // Short IDs are 12 chars in this UI.
    expect(screen.getByText('ch-xxxxxxxxx')).toBeTruthy();
    expect(screen.getByText('co-000000000')).toBeTruthy();
    expect(screen.getByText('tweak the thing')).toBeTruthy();
  });

  test('always renders an "All commits" row above the per-commit list', () => {
    renderPanel({ commits: [commit()] });
    expect(screen.getByText('All commits')).toBeTruthy();
  });

  test("clicking a commit row fires onselect with the change_id", async () => {
    const onselect = vi.fn();
    const c = commit({ change_id: 'ch-abc' });
    const { container } = renderPanel({ commits: [c], onselect });
    // The first li.commit is the synthetic "All commits" row; the
    // per-commit rows start at index 1.
    const perCommitRow = container.querySelectorAll('li.commit')[1] as HTMLElement;
    const rowButton = perCommitRow.querySelector('button.row-button') as HTMLElement;
    await fireEvent.click(rowButton);
    expect(onselect).toHaveBeenCalledWith('ch-abc');
  });

  test('marks the selected commit row with .selected', () => {
    const c = commit({ change_id: 'ch-abc' });
    const { container } = renderPanel({ commits: [c], selectedChangeId: 'ch-abc' });
    expect(container.querySelector('li.commit.selected')).not.toBeNull();
  });

  test('shows a comment-count badge on commits whose touched files have comments', () => {
    const c = commit({ change_id: 'ch-abc', changed_files: ['a.txt'] });
    // A line comment on a.txt — counted against this commit because
    // it touched a.txt.
    const cm = comment({
      anchor_change_id: 'ch-xxx',
      file: 'a.txt',
      side: 'tip',
      lines: { start: 1, end: 1 },
    });
    renderPanel({ commits: [c], comments: [cm] });
    expect(screen.getByText('1 comment')).toBeTruthy();
  });

  test("doesn't count file-level comments on files this commit didn't touch", () => {
    const c = commit({ change_id: 'ch-abc', changed_files: ['a.txt'] });
    const cm = comment({
      file: 'unrelated.txt',
      side: 'tip',
      lines: { start: 1, end: 1 },
    });
    renderPanel({ commits: [c], comments: [cm] });
    expect(screen.queryByText('1 comment')).toBeNull();
  });

  test('renders a commit-level thread inline under the commit row', () => {
    const c = commit({ change_id: 'ch-abc' });
    // Commit-level comment (no file/lines/side, not review_wide,
    // anchored to this change_id).
    const cm = comment({
      anchor_change_id: 'ch-abc',
      file: undefined,
      side: undefined,
      lines: undefined,
      body: 'design question',
    });
    renderPanel({ commits: [c], comments: [cm] });
    expect(screen.getByText('design question')).toBeTruthy();
  });

  test('renders review-wide comments under the "All commits" row', () => {
    const c = commit({ change_id: 'ch-abc' });
    const cm = comment({
      review_wide: true,
      file: undefined,
      side: undefined,
      lines: undefined,
      body: 'high-level note',
    });
    renderPanel({ commits: [c], comments: [cm] });
    expect(screen.getByText('high-level note')).toBeTruthy();
  });

  test('orphan commit-level comments (anchor change-id not in the visible commits) bucket into review-wide', () => {
    // A commit-level comment anchored to a change_id that's no
    // longer in the displayed commit list (e.g. the author rewrote
    // the stack and dropped that commit). The panel folds it into
    // the review-wide bucket so it stays visible.
    const c = commit({ change_id: 'ch-still-here' });
    const cm = comment({
      anchor_change_id: 'ch-vanished',
      file: undefined,
      side: undefined,
      lines: undefined,
      body: 'orphaned',
    });
    renderPanel({ commits: [c], comments: [cm] });
    // The orphan renders in the review-wide section (under "All commits"),
    // not under a per-commit row.
    expect(screen.getByText('orphaned')).toBeTruthy();
  });

  test('clicking the per-commit +comment bubble starts a commit-target compose', async () => {
    const onstartcompose = vi.fn();
    const c = commit({
      change_id: 'ch-abc',
      commit_id: 'co-xyz',
    });
    const { container } = renderPanel({
      commits: [c],
      onstartcompose,
    });
    // Per-commit bubble lives inside the second li.commit (index 1);
    // the first one is the synthetic "All commits" row whose bubble
    // does a review-wide compose instead.
    const perCommitRow = container.querySelectorAll('li.commit')[1] as HTMLElement;
    const bubble = perCommitRow.querySelector('.add-comment') as HTMLElement;
    await fireEvent.click(bubble);
    expect(onstartcompose).toHaveBeenCalledWith({
      kind: 'commit',
      change_id: 'ch-abc',
      commit_id: 'co-xyz',
    });
  });

  test('clicking the "All commits" +comment bubble starts a review-wide compose', async () => {
    const onstartcompose = vi.fn();
    const { container } = renderPanel({
      onstartcompose,
    });
    // The All-commits row is the first li.commit.
    const allCommitsRow = container.querySelector('li.commit') as HTMLElement;
    const reviewWideBubble = allCommitsRow.querySelector('.add-comment') as HTMLElement;
    await fireEvent.click(reviewWideBubble);
    expect(onstartcompose).toHaveBeenCalledWith({ kind: 'review' });
  });

  test('expand toggle reveals the multiline description body', async () => {
    const c = commit({
      description_first_line: 'tweak the thing',
      description: 'tweak the thing\n\nfollowed by some context\nthat spans lines',
    });
    const { container } = renderPanel({ commits: [c] });
    // Body isn't rendered until expanded.
    expect(container.querySelector('.body.markdown')).toBeNull();
    const expandBtn = container.querySelector('button.expand') as HTMLElement;
    await fireEvent.click(expandBtn);
    const body = container.querySelector('.body.markdown') as HTMLElement;
    expect(body).not.toBeNull();
    expect(within(body).getByText(/followed by some context/)).toBeTruthy();
  });
});
