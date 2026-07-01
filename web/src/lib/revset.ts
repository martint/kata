/**
 * Display helpers for revset strings shown in list / header chrome.
 *
 * GitHub-imported reviews use `<40-char-basesha>..<40-char-headsha>` as
 * their revset. In a review-list row that's ~85 chars of hex that
 * crowds the review title's ellipsis and reads as noise. The list
 * view doesn't need the full SHA — the review page has it if the
 * user cares — so we short-SHA any 40-char hex run to its first 9
 * chars, matching how jj / git surface short-SHAs in log output.
 *
 * We only touch runs that look like a full git SHA (exactly 40 hex
 * chars, case-insensitive, bounded by word boundaries). Symbolic
 * revsets (`main..feature`, `trunk()..@`) pass through untouched.
 */

/** Length to keep from each 40-char SHA. 9 matches jj log's default. */
const SHORT_SHA_LEN = 9;

/** Matches a full 40-char hex SHA at a word boundary — the shape both
 *  the GitHub importer (`<base>..<head>`) and manual commit-id
 *  revsets emit. Case-insensitive because git accepts either. */
const FULL_SHA = /\b[0-9a-f]{40}\b/gi;

export function shortenRevset(revset: string): string {
  return revset.replace(FULL_SHA, (sha) => sha.slice(0, SHORT_SHA_LEN));
}
