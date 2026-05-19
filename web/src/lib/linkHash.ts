//! Hash-fragment URL helpers for deep-linking into a review.
//!
//! The review viewer already understands `#c-<commentId>` and
//! `#file-<path>`; this module adds `#L:<file>:<side>:<start>[-<end>]`
//! so a free-form text selection in a diff can produce a stable
//! permalink the user can share in chat or paste into a commit
//! message.

/** Resolved line-range link. `endLine == startLine` for single-line
 *  selections (the hash form `#L:foo:tip:42` is sugar for
 *  `#L:foo:tip:42-42`). */
export interface LineRangeLink {
  file: string;
  side: 'base' | 'tip';
  startLine: number;
  endLine: number;
}

/** Build the hash fragment for a line range. Caller prefixes
 *  with `#` (or assigns to `window.location.hash`, which adds the
 *  `#` automatically). File path is URL-encoded so colons in the
 *  path don't confuse the parser; the rest of the segments are
 *  numeric or a fixed enum so they're safe verbatim. */
export function lineRangeHash(link: LineRangeLink): string {
  const file = encodeURIComponent(link.file);
  const range =
    link.startLine === link.endLine
      ? `${link.startLine}`
      : `${link.startLine}-${link.endLine}`;
  return `#L:${file}:${link.side}:${range}`;
}

/** Parse a hash fragment (with or without the leading `#`) into a
 *  line-range link. Returns `null` when the fragment isn't a line-
 *  range hash, malformed, or carries values outside the contract
 *  (negative lines, bad side enum, end < start). */
export function parseLineRangeHash(hash: string): LineRangeLink | null {
  const raw = hash.startsWith('#') ? hash.slice(1) : hash;
  if (!raw.startsWith('L:')) return null;
  // Split into at most 4 parts so a file path that contains an
  // (already-encoded) colon doesn't break the parse — though
  // `encodeURIComponent` always escapes `:` anyway, defence in depth.
  const parts = raw.slice(2).split(':');
  if (parts.length < 3) return null;
  const fileEncoded = parts.slice(0, parts.length - 2).join(':');
  const sideStr = parts[parts.length - 2];
  const rangeStr = parts[parts.length - 1];
  if (sideStr !== 'base' && sideStr !== 'tip') return null;
  const side = sideStr;

  let file: string;
  try {
    file = decodeURIComponent(fileEncoded);
  } catch {
    return null;
  }
  if (!file) return null;

  // `start` or `start-end` — both with positive integers.
  const dash = rangeStr.indexOf('-');
  const startStr = dash < 0 ? rangeStr : rangeStr.slice(0, dash);
  const endStr = dash < 0 ? rangeStr : rangeStr.slice(dash + 1);
  const startLine = Number(startStr);
  const endLine = Number(endStr);
  if (
    !Number.isInteger(startLine) ||
    !Number.isInteger(endLine) ||
    startLine <= 0 ||
    endLine <= 0 ||
    endLine < startLine
  ) {
    return null;
  }
  return { file, side, startLine, endLine };
}
