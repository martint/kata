//! Moved-line lookups for the diff renderers. The server marks blocks
//! of removed lines that reappear as added lines elsewhere in the same
//! diff (see `FileChange.moved`); this module answers "is this line
//! part of a moved block" per row and builds the label and link that
//! point at the block's other end.

import { lineRangeHash } from './linkHash';
import type { HunkLine, MovedLines, Side } from './types';

/** The moved block covering `line` on `side`, or null. */
export type MovedLookup = (side: Side, line: number | null | undefined) => MovedLines | null;

/** Index a file's moved blocks by side and line number. */
export function movedLookup(moved: MovedLines[] | undefined): MovedLookup {
  if (!moved || moved.length === 0) return () => null;
  const at = new Map<string, MovedLines>();
  for (const m of moved) {
    for (let n = m.lines.start; n <= m.lines.end; n++) at.set(`${m.side}:${n}`, m);
  }
  return (side, line) => (line == null ? null : (at.get(`${side}:${line}`) ?? null));
}

/** The moved block a hunk line belongs to. Context lines never move. */
export function movedBlockOf(lookup: MovedLookup, line: HunkLine): MovedLines | null {
  if (line.origin === 'removed') return lookup('base', line.base_line);
  if (line.origin === 'added') return lookup('tip', line.tip_line);
  return null;
}

/** The moved block that starts at `line`, for the label row above it. */
export function movedBlockStartingAt(lookup: MovedLookup, line: HunkLine | null): MovedLines | null {
  if (!line) return null;
  const m = movedBlockOf(lookup, line);
  const n = line.origin === 'removed' ? line.base_line : line.tip_line;
  return m && m.lines.start === n ? m : null;
}

/** Label naming the block's other end, e.g. "Code moved to util.py
 *  lines 3–8". "Code" keeps it apart from a comment's "moved" anchor
 *  badge. The other file shows by name only, so a deep path doesn't
 *  push the label off screen; `movedTitle` carries the full path. The
 *  name is left out when the block moved within `path`. */
export function movedLabel(m: MovedLines, path: string): string {
  return describeMove(m, path, m.other.path.split('/').pop() ?? m.other.path);
}

/** `movedLabel` with the other file's full path, for the tooltip. */
export function movedTitle(m: MovedLines, path: string): string {
  return describeMove(m, path, m.other.path);
}

function describeMove(m: MovedLines, path: string, otherName: string): string {
  const { start, end } = m.other.lines;
  const where = start === end ? `line ${start}` : `lines ${start}–${end}`;
  const file = m.other.path === path ? '' : `${otherName} `;
  return `${m.side === 'base' ? 'Code moved to' : 'Code moved from'} ${file}${where}`;
}

/** Line permalink to the block's other end. */
export function movedHref(m: MovedLines): string {
  return lineRangeHash({
    file: m.other.path,
    side: m.side === 'base' ? 'tip' : 'base',
    startLine: m.other.lines.start,
    endLine: m.other.lines.end,
  });
}

/** Identifies a moved block by both of its ends, so the removed end
 *  in one file and the added end in another produce the same key. */
export function moveKey(m: MovedLines, path: string): string {
  const here = `${path}:${m.lines.start}`;
  const there = `${m.other.path}:${m.other.lines.start}`;
  return m.side === 'base' ? `${here}>${there}` : `${there}>${here}`;
}

/** CSS classes for a row in a moved block. Empty for other rows. */
export function movedClass(m: MovedLines | null): string {
  if (!m) return '';
  return ` moved moved-${m.side === 'base' ? 'from' : 'to'}${m.alt ? ' moved-alt' : ''}`;
}
