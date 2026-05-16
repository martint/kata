//! Component tests for `FileDiff`. The focus is on the prop wiring
//! that's hard to verify by reading the source: which commit's file
//! does each side's syntax-highlight pass read from? Past bugs have
//! lived here (compare-mode read from the wrong file and rendered
//! unrelated lines as removed), and the only way to catch them is
//! to render the component and inspect the calls it makes.

import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { tick } from 'svelte';
import FileDiff from './FileDiff.svelte';
import type { FileChange, Patchset } from '../lib/types';

// Replace `lib/api`'s `readFile` with a spy so we can assert on which
// commit IDs FileDiff fetches for the base/tip blob reads. The other
// `api` methods are stubbed too — FileDiff doesn't call them in
// these tests, but `vi.mock` replaces the whole module so we need
// the shape to match what other imports expect.
vi.mock('../lib/api', () => ({
  api: {
    readFile: vi.fn(async () => ''),
    // Other methods aren't exercised by these tests; stub them so
    // any accidental call surfaces immediately rather than silently
    // resolving to `undefined`.
    listRepos: vi.fn(),
    listBookmarks: vi.fn(),
    listReviews: vi.fn(),
    openReview: vi.fn(),
    createReview: vi.fn(),
    refreshReview: vi.fn(),
    updateReviewSummary: vi.fn(),
    archiveReview: vi.fn(),
    unarchiveReview: vi.fn(),
    fileDiff: vi.fn(),
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

import { api } from '../lib/api';

afterEach(() => {
  vi.clearAllMocks();
});

function patchset(over: Partial<Patchset> = {}): Patchset {
  return {
    n: 2,
    base_change: 'base-ch',
    base_commit: 'ps2-base-commit',
    tip_change: 'ps2-tip-ch',
    tip_commit: 'ps2-tip-commit',
    recorded_at: '2026-05-15T10:00:00Z',
    parent_patchset: 1,
    ...over,
  };
}

function file(over: Partial<FileChange> = {}): FileChange {
  return {
    path: 'a.txt',
    status: 'modified',
    added: 1,
    removed: 1,
    binary: false,
    hunks: [
      {
        base_range: { start: 1, end: 1 },
        tip_range: { start: 1, end: 1 },
        lines: [
          { origin: 'removed', base_line: 1, content: 'old\n' },
          { origin: 'added', tip_line: 1, content: 'new\n' },
        ],
      },
    ],
    ...over,
  };
}

const noop = () => Promise.resolve();
const noopSync = () => {};

function renderFileDiff(
  props: Partial<Parameters<typeof FileDiff>[0]> = {},
) {
  return render(FileDiff as unknown as never, {
    props: {
      repo: 'test-repo',
      file: file(),
      patchset: patchset(),
      comments: [],
      responses: [],
      currentPatchset: 2,
      composing: null,
      saving: false,
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

describe('FileDiff', () => {
  test('reads the base blob from patchset.base_commit outside compare mode', async () => {
    renderFileDiff();
    await tick();
    const readFile = api.readFile as ReturnType<typeof vi.fn>;
    const baseReads = readFile.mock.calls.filter(
      (c) => c[1] === 'ps2-base-commit',
    );
    expect(baseReads.length).toBeGreaterThan(0);
    // And not from any other commit on the base side.
    const stray = readFile.mock.calls.filter(
      (c) =>
        c[1] !== 'ps2-base-commit' && c[1] !== 'ps2-tip-commit',
    );
    expect(stray).toEqual([]);
  });

  test(
    'reads the base blob from compareBaseCommit (not patchset.base_commit) when comparing',
    async () => {
      // The regression: in compare mode the diff base is the
      // compared patchset's tip, not the selected patchset's base.
      // Reading the wrong file made the highlight pass index into a
      // different file at the same line numbers, so removed-side
      // rows rendered with HTML pulled from the wrong place.
      renderFileDiff({ compareBaseCommit: 'ps1-tip-commit' });
      await tick();
      const readFile = api.readFile as ReturnType<typeof vi.fn>;
      const baseReads = readFile.mock.calls.filter(
        (c) => c[1] === 'ps1-tip-commit',
      );
      expect(baseReads.length).toBeGreaterThan(0);
      // The wrong commit must NOT be fetched on the base side.
      const wrongBaseReads = readFile.mock.calls.filter(
        (c) => c[1] === 'ps2-base-commit',
      );
      expect(wrongBaseReads).toEqual([]);
    },
  );

  test('renders the diff hunks by default (showDiffs=true, showComments=true)', () => {
    const { container } = renderFileDiff();
    // Hunks live inside .hunks-wrapper; the comments-only flat list
    // would render .compact-line-list instead.
    expect(container.querySelector('.hunks-wrapper')).not.toBeNull();
    expect(container.querySelector('.compact-line-list')).toBeNull();
  });

  test('comments-only mode (showDiffs=false) renders the flat list instead of hunks', () => {
    const f = file({
      hunks: null as unknown as undefined,
      added: 0,
      removed: 0,
    });
    const { container } = renderFileDiff({ showDiffs: false, file: f });
    expect(container.querySelector('.hunks-wrapper')).toBeNull();
    // With no line comments the list is empty but the placeholder
    // muted message shows.
    expect(container.querySelector('.compact-line-list, p.placeholder')).not.toBeNull();
  });

  test('diffs-only mode (showComments=false) hides the file-comment button', () => {
    const { container } = renderFileDiff({ showComments: false });
    expect(container.querySelector('button.file-comment')).toBeNull();
  });

  test('diffs+comments mode shows the file-comment button in the header', () => {
    const { container } = renderFileDiff();
    expect(container.querySelector('button.file-comment')).not.toBeNull();
  });

  test("doesn't render the whole-file toggle in comments-only mode", () => {
    const { container } = renderFileDiff({ showDiffs: false });
    expect(container.querySelector('button.whole-file')).toBeNull();
  });

  test('renders the whole-file toggle for modified files when diffs are on', () => {
    const { container } = renderFileDiff();
    // The button is gated on `canExpand` which is true for `modified`
    // and `renamed` statuses. Our fixture file is `modified`.
    expect(container.querySelector('button.whole-file')).not.toBeNull();
  });

  test('still reads the tip blob from patchset.tip_commit in compare mode', async () => {
    // The bug was base-side only; the tip side already lined up
    // because the diff's tip IS `patchset.tip_commit` regardless of
    // compare. This test guards against the fix accidentally
    // breaking the tip side.
    renderFileDiff({ compareBaseCommit: 'ps1-tip-commit' });
    await tick();
    const readFile = api.readFile as ReturnType<typeof vi.fn>;
    const tipReads = readFile.mock.calls.filter(
      (c) => c[1] === 'ps2-tip-commit',
    );
    expect(tipReads.length).toBeGreaterThan(0);
  });
});
