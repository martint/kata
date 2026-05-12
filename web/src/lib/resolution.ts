import type { ResolutionState, ResponseView } from './types';

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
