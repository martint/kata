//! Predicates and the cache-prune used by `ReviewViewer.refresh()`
//! when a new patchset auto-advances under the viewer.
//!
//! A new patchset can land while the user is composing a comment,
//! holds a per-file diff in the cache, or has a per-commit scope
//! open. Most of that state should reset (the cleanup mirrors what
//! `selectPatchset` does for the explicit transition); the pieces
//! here are the parts where survival depends on whether the
//! anchor still makes sense in the new view. Kept as pure functions
//! so they can be exercised without dragging `ReviewViewer`'s
//! state into a test.

import type {
  AnnotationView,
  CommentView,
  ComposerTarget,
  FileChange,
  CommitInfo,
} from './types';
import type { AnnotationComposerTarget } from '../components/AnnotationComposer.svelte';

/** Slice of `ReviewView` the survival predicates inspect. Typed as
 *  a structural minimum so callers can pass `current` directly,
 *  and so the tests can build a tiny fixture without populating
 *  every field of the full view. */
export interface SurvivalContext {
  diff: { files: Pick<FileChange, 'path'>[] };
  commits: Pick<CommitInfo, 'change_id'>[];
}

/** Decide whether an in-progress comment composer should survive a
 *  patchset auto-advance. The composer is kept iff its anchor is
 *  still meaningful in the new view:
 *
 *  - `'review'`: always survives — anchored to the review, not a
 *    file or commit.
 *  - `'line'` / `'file'`: file must still appear in the new diff.
 *    A patchset that deletes the file would leave the composer
 *    rendering against content that no longer exists.
 *  - `'commit'`: change-id must still appear in the new commits
 *    list. (Commit-IDs aren't checked — jj rewrites them on amend;
 *    the change-id is the stable identifier.) */
export function composerSurvivesPatchset(
  composing: ComposerTarget | null,
  view: SurvivalContext,
): ComposerTarget | null {
  if (!composing) return null;
  if (composing.kind === 'review') return composing;
  if (composing.kind === 'commit') {
    return view.commits.some((c) => c.change_id === composing.change_id)
      ? composing
      : null;
  }
  // 'line' or 'file' — both carry a file path.
  return view.diff.files.some((f) => f.path === composing.file)
    ? composing
    : null;
}

/** Same survival predicate for annotation composers. They support
 *  only `'line'` and `'file'` kinds, both file-anchored. */
export function annotationComposerSurvivesPatchset(
  composing: AnnotationComposerTarget | null,
  view: SurvivalContext,
): AnnotationComposerTarget | null {
  if (!composing) return null;
  return view.diff.files.some((f) => f.path === composing.file)
    ? composing
    : null;
}

/** Drop fileDiffCache entries that don't match the new
 *  `(patchset, compare)` selection. Keys are
 *  `${patchset}|${compare ?? ''}|${path}` for the normal flow and
 *  `interdiff|...` for per-commit interdiffs (see `FileSlot.svelte`
 *  for the exact construction). After a patchset auto-advance,
 *  old-patchset entries are unreachable because lookups use the
 *  new key; interdiff entries are also unreachable because the
 *  scoped state is reset alongside. Either way the entries are
 *  dead weight — pruning them stops a slow memory leak across
 *  long review sessions that span many refreshes.
 *
 *  Mutates `cache` in place. */
export function pruneFileDiffCache(
  cache: Map<string, unknown>,
  patchset: number,
  compare: number | null,
): void {
  const psPrefix = `${patchset}|${compare ?? ''}|`;
  for (const key of [...cache.keys()]) {
    if (!key.startsWith(psPrefix)) cache.delete(key);
  }
}

// `CommentView` / `AnnotationView` aren't used by the survival
// predicates but are re-exported here so importers can keep the
// patchset-swap surface in one place.
export type { CommentView, AnnotationView };
