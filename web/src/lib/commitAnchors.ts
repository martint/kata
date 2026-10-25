//! Places review threads in the per-commit view. The per-commit diff
//! carries each thread's anchor resolved against the viewed commit
//! (`CommitDiffView.anchors`).

import type { AnchorView, CommitDiffView, Side } from './types';

/** The fields of a comment or annotation that placement reads. */
export interface Placeable {
  anchor_commit_id: string;
  side?: Side;
  anchor: AnchorView;
}

/** `threads` with each anchor replaced by its position in the per-commit
 *  view of `diff`. `resolved` holds the anchors the server resolved
 *  against the commit, keyed by thread id. A thread written on this
 *  commit, on the side it sits on, keeps its stored lines. A thread
 *  missing from `resolved` was published after the diff loaded, and the
 *  view leaves it out until the next load. FileDiff then places each
 *  thread inline, with the off-diff threads or as outdated, as it does
 *  in the full-review view. */
export function threadsInCommit<T extends Placeable>(
  threads: T[],
  idOf: (thread: T) => string,
  resolved: Record<string, AnchorView> | undefined,
  diff: CommitDiffView,
): T[] {
  const out: T[] = [];
  for (const t of threads) {
    const own = t.side === 'base' ? diff.base_commit : diff.tip_commit;
    const anchor: AnchorView | undefined =
      t.anchor_commit_id === own ? { kind: 'valid' } : resolved?.[idOf(t)];
    if (anchor) out.push({ ...t, anchor });
  }
  return out;
}
