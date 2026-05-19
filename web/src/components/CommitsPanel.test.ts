//! Component tests for `CommitsPanel`. Covers the per-commit row
//! rendering, comment-count badges over file membership, the
//! "All commits" affordance for review-wide comments, scoped vs
//! unscoped selection, and the commit-level / review-wide inline
//! comment threads that the recent commit-comment work added.

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import type { ComponentProps } from 'svelte';
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
  props: Partial<ComponentProps<typeof CommitsPanel>> = {},
) {
  return render(CommitsPanel, {
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

// ---- Patchset-compare v2: compare-mode rendering -----------------------
//
// Covers the side-panel surface that activates when `compareView` is set:
// pair list with badges, summary chip, click-through, base-mismatch
// banner, hide-same toggle, error surfacing, and the writeability gate.

import type { PatchsetCompareView, PatchsetPair } from '../lib/types';

function pair(over: Partial<PatchsetPair> = {}): PatchsetPair {
  return {
    change_id: 'ch-pair-1',
    status: 'changed',
    from_commit: 'co-old',
    to_commit: 'co-new',
    from_description: 'tweak v1',
    to_description: 'tweak v2',
    ...over,
  };
}

function compareView(
  over: Partial<PatchsetCompareView> = {},
): PatchsetCompareView {
  return {
    from: { n: 1, base_commit: 'co-base', tip_commit: 'co-from-tip' },
    to: { n: 2, base_commit: 'co-base', tip_commit: 'co-to-tip' },
    cumulative: { base: 'co-base', tip: 'co-to-tip', files: [] },
    pairs: [pair()],
    compare_base_mismatch: false,
    ...over,
  };
}

describe('CommitsPanel — compare mode', () => {
  test('renders one row per pair with the right status badge', () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({ change_id: 'ch-changed', status: 'changed' }),
          pair({ change_id: 'ch-same', status: 'same' }),
          pair({
            change_id: 'ch-added',
            status: 'added-in-to',
            from_commit: undefined,
            from_description: undefined,
            parent_commit: 'co-parent',
          }),
          pair({
            change_id: 'ch-removed',
            status: 'removed-from-from',
            to_commit: undefined,
            to_description: undefined,
            parent_commit: 'co-parent',
          }),
        ],
      }),
    });
    // Pair rows live under .compare-pairs; the first li is the
    // "Cumulative" sentinel, then one per pair.
    const rows = Array.from(
      container.querySelectorAll('ul.compare-pairs li.pair-row'),
    );
    // 1 sentinel + 4 pair entries.
    expect(rows.length).toBe(5);
    // Status badges drop through their class name; check the
    // status-XXX class for each row.
    const statuses = rows
      .slice(1)
      .map((r) => Array.from(r.classList).find((c) => c.startsWith('status-')));
    expect(statuses).toEqual([
      'status-changed',
      'status-same',
      'status-added-in-to',
      'status-removed-from-from',
    ]);
  });

  test('renders the summary chip with status counts', () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({ change_id: 'a', status: 'changed' }),
          pair({ change_id: 'b', status: 'changed' }),
          pair({ change_id: 'c', status: 'same' }),
        ],
      }),
    });
    const summary = container.querySelector('.compare-summary')!;
    expect(summary.textContent).toContain('2 changed');
    expect(summary.textContent).toContain('1 same');
  });

  test('clicking a changed pair fires onselectcomparecommit with its change_id', async () => {
    const onselectcomparecommit = vi.fn();
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [pair({ change_id: 'ch-clickme', status: 'changed' })],
      }),
      onselectcomparecommit,
    });
    // The first li.pair-row is the Cumulative sentinel; the actual
    // pair row is at index 1.
    const pairRow = container.querySelectorAll('li.pair-row')[1] as HTMLElement;
    const btn = pairRow.querySelector('button.row-button') as HTMLElement;
    await fireEvent.click(btn);
    expect(onselectcomparecommit).toHaveBeenCalledWith('ch-clickme');
  });

  test('clicking the Cumulative sentinel fires onselectcomparecommit(null)', async () => {
    const onselectcomparecommit = vi.fn();
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [pair()],
      }),
      selectedCompareChange: 'ch-pair-1',
      onselectcomparecommit,
    });
    const sentinel = container.querySelector('li.pair-row') as HTMLElement;
    const btn = sentinel.querySelector('button.row-button') as HTMLElement;
    await fireEvent.click(btn);
    expect(onselectcomparecommit).toHaveBeenCalledWith(null);
  });

  test('same and incomplete add/remove rows are inert (button disabled)', () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({ change_id: 'a', status: 'same' }),
          // added-in-to with no parent_commit → inert
          pair({
            change_id: 'b',
            status: 'added-in-to',
            from_commit: undefined,
            from_description: undefined,
          }),
        ],
      }),
    });
    const rows = container.querySelectorAll('ul.compare-pairs li.pair-row');
    // Skip the Cumulative sentinel (index 0).
    for (let i = 1; i < rows.length; i++) {
      const btn = rows[i].querySelector('button.row-button') as HTMLButtonElement;
      expect(btn.disabled).toBe(true);
      expect(rows[i].classList.contains('inert')).toBe(true);
    }
  });

  test('added/removed rows become clickable when parent_commit is present', () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({
            change_id: 'a',
            status: 'added-in-to',
            from_commit: undefined,
            from_description: undefined,
            parent_commit: 'co-parent',
          }),
          pair({
            change_id: 'b',
            status: 'removed-from-from',
            to_commit: undefined,
            to_description: undefined,
            parent_commit: 'co-parent',
          }),
        ],
      }),
    });
    const rows = container.querySelectorAll(
      'ul.compare-pairs li.pair-row.clickable',
    );
    expect(rows.length).toBe(2);
  });

  test('description-delta chip renders when from_description != to_description', () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({
            from_description: 'old subject',
            to_description: 'new subject',
          }),
        ],
      }),
    });
    expect(container.querySelector('.desc-delta')).not.toBeNull();
  });

  test('description-delta chip does NOT render when descriptions match', () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({
            from_description: 'same subject',
            to_description: 'same subject',
          }),
        ],
      }),
    });
    expect(container.querySelector('.desc-delta')).toBeNull();
  });

  test('pair-counts chip renders when diff_counts is populated', () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({
            diff_counts: { file_count: 3, added: 7, removed: 15 },
          }),
        ],
      }),
    });
    const counts = container.querySelector('.pair-counts')!;
    expect(counts.textContent).toMatch(/3f/);
    expect(counts.textContent).toMatch(/\+7/);
    expect(counts.textContent).toMatch(/−15|-15/);
  });

  test('base-mismatch banner renders the commit prefixes', () => {
    const { container } = renderPanel({
      compareView: compareView({
        compare_base_mismatch: true,
        from: { n: 1, base_commit: 'fromffffffffaaaa', tip_commit: 't1' },
        to: { n: 2, base_commit: 'toobbbbbbbbcccc', tip_commit: 't2' },
      }),
    });
    const banner = container.querySelector('.compare-warn')!;
    expect(banner.textContent).toContain('fromffffffff');
    expect(banner.textContent).toContain('toobbbbbbbb');
  });

  test('hide-same toggle filters status=same rows', async () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({ change_id: 'a', status: 'changed' }),
          pair({ change_id: 'b', status: 'same' }),
          pair({ change_id: 'c', status: 'same' }),
        ],
      }),
    });
    // Before toggle: 1 sentinel + 3 pairs = 4 rows.
    expect(container.querySelectorAll('li.pair-row').length).toBe(4);
    const toggle = container.querySelector(
      '.hide-same-toggle input',
    ) as HTMLInputElement;
    await fireEvent.click(toggle);
    // After: 1 sentinel + 1 non-same = 2 rows.
    expect(container.querySelectorAll('li.pair-row').length).toBe(2);
  });

  test('compareError banner renders when set', () => {
    const { container } = renderPanel({
      compareError: 'patchset 3 not found',
    });
    const err = container.querySelector('.compare-error')!;
    expect(err.textContent).toContain('patchset 3 not found');
  });

  test('commentsWriteable=false hides the review-wide + button in compare mode', () => {
    const { container } = renderPanel({
      commits: [commit()],
      compareView: compareView(),
      commentsWriteable: false,
    });
    // No add-comment buttons anywhere in the panel.
    expect(container.querySelectorAll('button.add-comment').length).toBe(0);
  });

  test('commentsWriteable=true keeps add-comment buttons present', () => {
    const { container } = renderPanel({
      commits: [commit()],
      compareView: compareView(),
      commentsWriteable: true,
    });
    expect(
      container.querySelectorAll('button.add-comment').length,
    ).toBeGreaterThan(0);
  });

  test('in compare mode, the panel renders the pair list and a Review-wide block (no duplicate commit list)', () => {
    const { container } = renderPanel({
      commits: [commit()],
      compareView: compareView(),
    });
    // Pair list present.
    expect(container.querySelector('ul.compare-pairs')).not.toBeNull();
    // Review-wide block present.
    expect(container.querySelector('.review-wide-block')).not.toBeNull();
    // Only ONE ul in the panel: the compare-pairs list. The
    // non-compare-mode commit list is suppressed so we don't
    // duplicate "all the commits" twice in compare mode.
    expect(container.querySelectorAll('ul.commit-list').length).toBe(1);
  });

  test('commit-level threads anchored to a pair attach inline under that pair row', () => {
    const c = comment({
      anchor_change_id: 'ch-pair-1',
      file: undefined,
      side: undefined,
      lines: undefined,
      body: 'commit-level note',
    });
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [pair({ change_id: 'ch-pair-1' })],
      }),
      comments: [c],
    });
    // The thread renders inside the pair row's .commit-threads block,
    // not in the Review-wide block.
    const pairRow = container.querySelectorAll('li.pair-row')[1] as HTMLElement;
    expect(pairRow.querySelector('.commit-threads')).not.toBeNull();
    expect(pairRow.textContent).toContain('commit-level note');
    // Review-wide block exists (header is always there when
    // writeable) but doesn't carry this thread.
    const reviewWide = container.querySelector('.review-wide-block')!;
    expect(reviewWide.textContent).not.toContain('commit-level note');
  });

  test('orphan commit-level threads (no matching pair) fall into the Review-wide block', () => {
    const c = comment({
      anchor_change_id: 'ch-not-in-pairs',
      file: undefined,
      side: undefined,
      lines: undefined,
      body: 'orphan note',
    });
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [pair({ change_id: 'ch-pair-1' })],
      }),
      comments: [c],
    });
    const reviewWide = container.querySelector('.review-wide-block')!;
    expect(reviewWide.textContent).toContain('orphan note');
  });

  test('changed pair with zero-file rebased diff is rendered as "rebased only"', () => {
    // Use matching descriptions on both rebased pairs so the
    // "description-only delta" exception doesn't trigger — that path
    // keeps a pair as `changed` even when content is identical.
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({
            change_id: 'ch-rebased',
            status: 'changed',
            from_description: 'same subject',
            to_description: 'same subject',
            diff_counts: { file_count: 0, added: 0, removed: 0 },
          }),
          pair({
            change_id: 'ch-actual',
            status: 'changed',
            from_description: 'same subject',
            to_description: 'same subject',
            diff_counts: { file_count: 3, added: 7, removed: 2 },
          }),
        ],
      }),
    });
    const rows = container.querySelectorAll('ul.compare-pairs li.pair-row');
    // Index 0 is the Cumulative sentinel.
    const rebased = rows[1] as HTMLElement;
    const actual = rows[2] as HTMLElement;
    expect(rebased.classList.contains('rebased-only')).toBe(true);
    expect(actual.classList.contains('rebased-only')).toBe(false);
    // The rebased row's status label reads "rebased only", not "changed".
    expect(rebased.textContent).toContain('rebased only');
    expect(actual.textContent).toContain('changed');
    // The rebased row also drops the pair-counts chip (0/0/0 is
    // meaningless info).
    expect(rebased.querySelector('.pair-counts')).toBeNull();
    expect(actual.querySelector('.pair-counts')).not.toBeNull();
  });

  test('summary chip splits "rebased only" out of the "changed" count', () => {
    // Same caveat as above: matching descriptions so descChanged
    // doesn't override the rebased-only classification.
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({
            change_id: 'a',
            status: 'changed',
            from_description: 'x',
            to_description: 'x',
            diff_counts: { file_count: 0, added: 0, removed: 0 },
          }),
          pair({
            change_id: 'b',
            status: 'changed',
            from_description: 'x',
            to_description: 'x',
            diff_counts: { file_count: 0, added: 0, removed: 0 },
          }),
          pair({
            change_id: 'c',
            status: 'changed',
            from_description: 'x',
            to_description: 'x',
            diff_counts: { file_count: 1, added: 1, removed: 1 },
          }),
        ],
      }),
    });
    const summary = container.querySelector('.compare-summary')!;
    expect(summary.textContent).toMatch(/1 changed/);
    expect(summary.textContent).toMatch(/2 rebased/);
  });

  test('a "changed" pair with description-only delta (no content) stays as "changed"', () => {
    const { container } = renderPanel({
      compareView: compareView({
        pairs: [
          pair({
            change_id: 'ch-desc',
            status: 'changed',
            from_description: 'old subject',
            to_description: 'new subject',
            diff_counts: { file_count: 0, added: 0, removed: 0 },
          }),
        ],
      }),
    });
    const row = container.querySelectorAll('li.pair-row')[1] as HTMLElement;
    // Not rebased-only: description differs, so the pair is a real
    // (if metadata-only) rewrite.
    expect(row.classList.contains('rebased-only')).toBe(false);
    expect(row.querySelector('.desc-delta')).not.toBeNull();
  });

  test('review-wide threads (review_wide: true) fall into the Review-wide block', () => {
    const c = comment({
      review_wide: true,
      file: undefined,
      side: undefined,
      lines: undefined,
      body: 'whole-review note',
    });
    const { container } = renderPanel({
      compareView: compareView(),
      comments: [c],
    });
    const reviewWide = container.querySelector('.review-wide-block')!;
    expect(reviewWide.textContent).toContain('whole-review note');
  });
});
