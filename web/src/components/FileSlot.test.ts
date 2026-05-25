//! Component tests for `FileSlot`. The focus is on the
//! file-hunks-vs-cache priority: which hunks does the slot hand to
//! `FileDiff` when both an inline `file.hunks` and a cached entry at
//! the same `(patchset, compare, path)` key exist? Past bugs have
//! lived here — a stale unscoped fetch resurrected itself under a
//! scoped commit view because the cache key has no scope component,
//! and the diff for one file rendered with hunks from a different
//! endpoint pair.

import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { tick, type ComponentProps } from 'svelte';
import FileSlot from './FileSlot.svelte';
import type { FileChange, Hunk, Patchset } from '../lib/types';

// Stub `lib/api` so the per-file fetch path is observable. We don't
// want a real network call escaping into the test runner.
vi.mock('../lib/api', () => ({
  api: {
    readFile: vi.fn(async () => ''),
    fileDiff: vi.fn(),
    diffCommits: vi.fn(),
    // Round out the shape so any accidental call surfaces rather than
    // silently resolving to `undefined`.
    listRepos: vi.fn(),
    listBookmarks: vi.fn(),
    listReviews: vi.fn(),
    openReview: vi.fn(),
    createReview: vi.fn(),
    refreshReview: vi.fn(),
    updateReviewSummary: vi.fn(),
    archiveReview: vi.fn(),
    unarchiveReview: vi.fn(),
    startSession: vi.fn(),
    publishSession: vi.fn(),
    discardSession: vi.fn(),
    draftComment: vi.fn(),
    updateDraftComment: vi.fn(),
    deleteDraftComment: vi.fn(),
    respond: vi.fn(),
    commitDiff: vi.fn(),
    previewRevset: vi.fn(),
  },
}));

afterEach(() => {
  vi.clearAllMocks();
});

function patchset(over: Partial<Patchset> = {}): Patchset {
  return {
    n: 5,
    base_change: 'base-ch',
    base_commit: 'ps5-base-commit',
    tip_change: 'ps5-tip-ch',
    tip_commit: 'ps5-tip-commit',
    recorded_at: '2026-05-15T10:00:00Z',
    parent_patchset: 4,
    ...over,
  };
}

function hunk(label: string): Hunk {
  return {
    kind: 'regular',
    base_range: { start: 1, end: 1 },
    tip_range: { start: 1, end: 1 },
    lines: [
      { origin: 'removed', base_line: 1, content: `old-${label}\n` },
      { origin: 'added', tip_line: 1, content: `new-${label}\n` },
    ],
  };
}

function file(over: Partial<FileChange> = {}): FileChange {
  return {
    path: 'a.txt',
    status: 'modified',
    added: 1,
    removed: 1,
    binary: false,
    hunks: [hunk('inline')],
    ...over,
  };
}

const noop = () => Promise.resolve();
const noopSync = () => {};

function renderSlot(
  props: Partial<ComponentProps<typeof FileSlot>> = {},
) {
  return render(FileSlot, {
    props: {
      repo: 'test-repo',
      reviewNumber: 1,
      file: file(),
      patchset: patchset(),
      compareWith: null,
      compareBaseCommit: null,
      // Force the inner FileDiff to mount synchronously — jsdom's
      // IntersectionObserver stub never fires, so without this the
      // slot would stay as a placeholder and we couldn't inspect
      // what `effectiveFile` resolved to.
      forceRender: true,
      eagerFetch: false,
      comments: [],
      responses: [],
      currentPatchset: 5,
      composing: null,
      saving: false,
      showDiffs: true,
      showComments: true,
      sbsSplit: 0.5,
      setSbsSplit: noopSync,
      diffCache: new Map(),
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

describe('FileSlot effectiveFile', () => {
  test('prefers inline hunks over a stale cache entry at the same key', async () => {
    // The regression: scoping a review down to a single commit feeds
    // FileSlot a file whose hunks are inlined from `commit_diff`.
    // The cache key `${patchset.n}|${compareWith}|${path}` has no
    // scope component, so a previous unscoped render at the same
    // patchset would have populated the cache with hunks for a
    // different endpoint pair. Without this guard, `effectiveFile`
    // resurrects the cached entry and discards the inline hunks —
    // the user sees the wrong diff for a single file.
    const cache = new Map<string, FileChange>();
    cache.set('5||a.txt', file({ hunks: [hunk('cached-stale')] }));
    const { container } = renderSlot({
      file: file({ hunks: [hunk('scoped')] }),
      diffCache: cache,
    });
    await tick();
    const text = container.textContent ?? '';
    expect(text).toContain('new-scoped');
    expect(text).not.toContain('new-cached-stale');
  });

  test('falls back to the cache when the file is metadata-only', async () => {
    // The cache exists to bridge the metadata-only flow across slot
    // virtualization remounts: `open_review` ships files without
    // hunks, FileSlot fetches them lazily, the result lands in the
    // cache. Once cached, a remount of the same slot must render the
    // cached hunks rather than refetching. Guard against the fix
    // above breaking that path.
    const cache = new Map<string, FileChange>();
    cache.set('5||a.txt', file({ hunks: [hunk('cached')] }));
    const { container } = renderSlot({
      file: file({ hunks: null as unknown as undefined }),
      diffCache: cache,
    });
    await tick();
    const text = container.textContent ?? '';
    expect(text).toContain('new-cached');
  });
});
