//! Tests for the helpers `ReviewViewer.refresh()` uses to decide
//! what survives a new-patchset auto-advance. Each helper is pure —
//! survival is a function of the (composer, view) pair; the cache
//! prune is a single pass over the map's keys — so the tests just
//! exercise data shapes without spinning the component.

import { describe, expect, test } from 'vitest';
import {
  annotationComposerSurvivesPatchset,
  composerSurvivesPatchset,
  pruneFileDiffCache,
  type SurvivalContext,
} from './patchsetSwap';
import type { AnnotationComposerTarget } from '../components/AnnotationComposer.svelte';
import type { ComposerTarget } from './types';

function view(over: Partial<SurvivalContext> = {}): SurvivalContext {
  return {
    diff: { files: [{ path: 'a.txt' }, { path: 'b.txt' }] },
    commits: [{ change_id: 'abc' }, { change_id: 'def' }],
    ...over,
  };
}

describe('composerSurvivesPatchset', () => {
  test('null stays null', () => {
    expect(composerSurvivesPatchset(null, view())).toBeNull();
  });

  test("'review' composer always survives — no file or commit anchor", () => {
    const c: ComposerTarget = { kind: 'review' };
    expect(composerSurvivesPatchset(c, view({ diff: { files: [] }, commits: [] }))).toBe(c);
  });

  test("'line' composer kept when its file is still in the new diff", () => {
    const c: ComposerTarget = {
      kind: 'line',
      file: 'a.txt',
      side: 'tip',
      startLine: 3,
      endLine: 5,
    };
    expect(composerSurvivesPatchset(c, view())).toBe(c);
  });

  test("'line' composer cancelled when its file got deleted in the new diff", () => {
    const c: ComposerTarget = {
      kind: 'line',
      file: 'gone.txt',
      side: 'tip',
      startLine: 1,
      endLine: 1,
    };
    expect(composerSurvivesPatchset(c, view())).toBeNull();
  });

  test("'file' composer follows the same file-presence rule", () => {
    const kept: ComposerTarget = { kind: 'file', file: 'b.txt' };
    const cancelled: ComposerTarget = { kind: 'file', file: 'gone.txt' };
    expect(composerSurvivesPatchset(kept, view())).toBe(kept);
    expect(composerSurvivesPatchset(cancelled, view())).toBeNull();
  });

  test("'commit' composer kept when its change-id is in the new commits list", () => {
    const c: ComposerTarget = { kind: 'commit', change_id: 'abc', commit_id: 'co1' };
    expect(composerSurvivesPatchset(c, view())).toBe(c);
  });

  test("'commit' composer cancelled when its change-id dropped out of the new revset", () => {
    const c: ComposerTarget = { kind: 'commit', change_id: 'zzz', commit_id: 'old' };
    expect(composerSurvivesPatchset(c, view())).toBeNull();
  });

  test('survival is keyed on change-id, not commit-id — commit IDs change on amend', () => {
    // The same change-id exists in the new view but the commit hash
    // is different (typical after a `jj amend`). The composer's
    // anchor_commit_id will be rewritten by the backend on submit
    // via the same re-anchor path Outdated comments use; what
    // matters here is that the change still corresponds to a
    // visible commit, which it does.
    const c: ComposerTarget = { kind: 'commit', change_id: 'def', commit_id: 'old-hash' };
    expect(composerSurvivesPatchset(c, view())).toBe(c);
  });
});

describe('annotationComposerSurvivesPatchset', () => {
  test('null stays null', () => {
    expect(annotationComposerSurvivesPatchset(null, view())).toBeNull();
  });

  test("'line' annotation kept when file still present", () => {
    const a: AnnotationComposerTarget = {
      kind: 'line',
      file: 'a.txt',
      side: 'tip',
      startLine: 1,
      endLine: 1,
    };
    expect(annotationComposerSurvivesPatchset(a, view())).toBe(a);
  });

  test("'file' annotation cancelled when file dropped", () => {
    const a: AnnotationComposerTarget = { kind: 'file', file: 'gone.txt' };
    expect(annotationComposerSurvivesPatchset(a, view())).toBeNull();
  });
});

describe('pruneFileDiffCache', () => {
  test('keeps entries that match the new (patchset, compare) prefix', () => {
    const cache = new Map<string, unknown>([
      ['2||a.txt', 'A'],
      ['2||b.txt', 'B'],
    ]);
    pruneFileDiffCache(cache, 2, null);
    expect([...cache.keys()].sort()).toEqual(['2||a.txt', '2||b.txt']);
  });

  test('drops entries from a previous patchset selection', () => {
    const cache = new Map<string, unknown>([
      ['1||a.txt', 'old'],
      ['1||b.txt', 'old'],
      ['2||a.txt', 'new'],
    ]);
    pruneFileDiffCache(cache, 2, null);
    expect([...cache.keys()]).toEqual(['2||a.txt']);
  });

  test('compare param participates in the prefix', () => {
    // Same patchset (2) but different compare values; only the
    // currently-selected (2, compare=1) entries survive.
    const cache = new Map<string, unknown>([
      ['2|1|a.txt', 'compare-1'],
      ['2|3|a.txt', 'compare-3'],
      ['2||a.txt', 'no-compare'],
    ]);
    pruneFileDiffCache(cache, 2, 1);
    expect([...cache.keys()]).toEqual(['2|1|a.txt']);
  });

  test('interdiff-keyed entries are pruned (scoped state resets alongside)', () => {
    const cache = new Map<string, unknown>([
      ['interdiff|p|abc|def|a.txt', 'old-pair'],
      ['2||a.txt', 'new'],
    ]);
    pruneFileDiffCache(cache, 2, null);
    expect([...cache.keys()]).toEqual(['2||a.txt']);
  });

  test('empty cache stays empty', () => {
    const cache = new Map<string, unknown>();
    pruneFileDiffCache(cache, 2, null);
    expect(cache.size).toBe(0);
  });
});
