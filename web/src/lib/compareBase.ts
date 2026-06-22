//! The commit a compare-mode diff's `base_line` numbers index into —
//! used by `ReviewViewer` to tell `FileDiff` which file version to
//! read for the base side's syntax-highlight pass and context reads.
//!
//! Compare mode has two shapes, and they disagree on what "the base"
//! is. Getting it wrong doesn't fail loudly — it just highlights the
//! removed side from the wrong file version, so the base column shows
//! text from unrelated lines under the diff's (correct) line numbers.
//! Kept pure so the choice can be exercised without `ReviewViewer`'s
//! state.

/** Structural minimum of a patchset the chooser inspects. */
export interface PatchsetTip {
  n: number;
  tip_commit: string;
}

/**
 * The base commit a compare-mode diff is computed against, or `null`
 * outside compare mode.
 *
 * @param interdiffFrom The from-side commit of the selected per-commit
 *   pair (`interdiffEndpoints.from`), or `null` when no pair is
 *   selected. For a per-commit interdiff this is the real base — the
 *   parent for an added/removed pair, the from-side commit for a
 *   changed pair — and it takes precedence: the compared patchset's
 *   tip would point at an entirely different file.
 * @param compareWith The patchset number being compared against, or
 *   `null` outside compare mode.
 * @param patchsets The review's patchsets, to resolve `compareWith`'s
 *   tip for the whole-patchset cumulative compare.
 */
export function compareBaseCommit(
  interdiffFrom: string | null,
  compareWith: number | null,
  patchsets: PatchsetTip[],
): string | null {
  if (interdiffFrom != null) return interdiffFrom;
  if (compareWith == null) return null;
  return patchsets.find((p) => p.n === compareWith)?.tip_commit ?? null;
}
