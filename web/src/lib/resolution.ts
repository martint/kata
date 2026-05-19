import type { FoldStore } from './foldStore';
import type { ResolutionState, ResponseView } from './types';

/** Per-thread default fold state for the new fold model. A thread is
 *  folded by default when the global view mode says so OR when its
 *  resolution is anything other than `open`. The user can override
 *  per-thread via `foldStore` (kind `comment`, keyed by comment_id) —
 *  resolution-aware default is just the value the store falls back to
 *  when the user hasn't touched a thread explicitly.
 *
 *  `defaultThreadsCollapsed` mirrors the view-mode default (true in
 *  Compact, false in Full); resolution wins the tie in Full mode by
 *  folding resolved threads even though everything else is expanded. */
export function defaultFoldedForThread(
  commentId: string,
  responses: ResponseView[],
  defaultThreadsCollapsed: boolean,
): boolean {
  if (defaultThreadsCollapsed) return true;
  return resolutionFor(commentId, responses) !== 'open';
}

/** Effective fold state for one thread: the user's persisted choice
 *  (if any), else the resolution-aware default. Used by both the
 *  per-thread fold control inside `CommentThread` and the aggregate
 *  gutter marker in `HunkLines` / `HunkLinesSideBySide`. */
export function isThreadFolded(
  commentId: string,
  responses: ResponseView[],
  foldStore: FoldStore | undefined,
  defaultThreadsCollapsed: boolean,
): boolean {
  const stored = foldStore?.get('comment', commentId);
  if (typeof stored === 'boolean') return stored;
  return defaultFoldedForThread(commentId, responses, defaultThreadsCollapsed);
}

/** True when the thread `commentId` has at least one published
 *  response newer than the viewer's last visit (and not authored by
 *  the viewer). Used to force-expand folded threads so a fresh
 *  response can't hide behind a fold set by the responder. */
export function hasUnreadReplies(
  commentId: string,
  responses: ResponseView[],
  lastVisitAt: string | null,
  viewer: string,
): boolean {
  if (!lastVisitAt) return false;
  return responses.some(
    (r) =>
      r.in_reply_to === commentId &&
      !r.draft &&
      r.author !== viewer &&
      r.created_at > lastVisitAt,
  );
}

/** Replay a comment's responses (status-changing only) and return the
 *  resolution state. Last status action wins. Drafts are included so the
 *  author sees the effect their unpublished response will have. */
export function resolutionFor(
  commentId: string,
  responses: ResponseView[],
): ResolutionState {
  const status = responses
    .filter((r) => r.in_reply_to === commentId && r.action !== 'comment')
    .slice()
    .sort((a, b) => a.created_at.localeCompare(b.created_at));
  if (status.length === 0) return 'open';
  const last = status[status.length - 1].action;
  switch (last) {
    case 'resolve':
      return 'resolved';
    case 'wont-fix':
      return 'wont-fix';
    case 'unresolve':
      return 'open';
    default:
      return 'open';
  }
}
