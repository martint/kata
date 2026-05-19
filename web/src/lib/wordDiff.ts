//! Word-level diff and HTML overlay for paired changed lines.
//!
//! Pairing strategy comes from `hunkAlign.alignBlock` — a
//! Needleman–Wunsch alignment over (remove, add) line-similarity
//! scores. N:N blocks of similar lines pair straight through (same
//! visual as the old strict-index zip); uneven or partially-changed
//! blocks find their best content matches and leave the rest
//! one-sided. Pre-alignment we bailed entirely on anything other
//! than strict N:N.

import { alignBlock } from './hunkAlign';
//!
//! The line-level diff already tells the reader *that* a line changed;
//! this layer says *what* inside the line changed. We tokenize each
//! line into runs of word characters and non-word characters, run a
//! Myers diff over the token stream, and emit ranges (in original
//! character offsets) that the renderer wraps with a stronger
//! background tint on top of the syntax-highlighted HTML.

/** A run of consecutive changed characters in one side of a line. */
export interface WordDiffRange {
  /** UTF-16 character offset (inclusive) into the line's original text. */
  start: number;
  /** UTF-16 character offset (exclusive). */
  end: number;
}

/** Computed word-diff ranges for a paired (remove, add) line. */
export interface LineWordDiff {
  /** Ranges in the remove line's text that exist in remove but not add. */
  removed: WordDiffRange[];
  /** Ranges in the add line's text that exist in add but not remove. */
  added: WordDiffRange[];
}

interface Token {
  text: string;
  start: number;
  end: number;
}

/** Split a line into tokens: maximal runs of `\w` characters alternated
 *  with single non-word characters. The single-char policy for
 *  punctuation/whitespace gives finer-grained diff output (e.g. an added
 *  `,` doesn't have to drag the surrounding space along with it). */
function tokenize(line: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < line.length) {
    const ch = line.charCodeAt(i);
    const isWord =
      (ch >= 48 && ch <= 57) || // 0-9
      (ch >= 65 && ch <= 90) || // A-Z
      (ch >= 97 && ch <= 122) || // a-z
      ch === 95 || // _
      ch > 127; // non-ASCII (treat as word so identifiers in non-Latin scripts stay together)
    if (isWord) {
      const start = i;
      while (i < line.length) {
        const c = line.charCodeAt(i);
        const w =
          (c >= 48 && c <= 57) ||
          (c >= 65 && c <= 90) ||
          (c >= 97 && c <= 122) ||
          c === 95 ||
          c > 127;
        if (!w) break;
        i++;
      }
      tokens.push({ text: line.slice(start, i), start, end: i });
    } else {
      tokens.push({ text: line[i], start: i, end: i + 1 });
      i++;
    }
  }
  return tokens;
}

/** Classic LCS table for two token arrays. Returns the table so the
 *  caller can walk back through it. */
function lcsTable(a: Token[], b: Token[]): Uint32Array {
  const m = a.length;
  const n = b.length;
  // Row-major (m+1) x (n+1). Uint32 is plenty for the line lengths we deal with.
  const dp = new Uint32Array((m + 1) * (n + 1));
  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      const idx = i * (n + 1) + j;
      if (a[i - 1].text === b[j - 1].text) {
        dp[idx] = dp[(i - 1) * (n + 1) + (j - 1)] + 1;
      } else {
        const up = dp[(i - 1) * (n + 1) + j];
        const left = dp[i * (n + 1) + (j - 1)];
        dp[idx] = up >= left ? up : left;
      }
    }
  }
  return dp;
}

/** Walk back through the LCS table to label each token in `a` and `b`
 *  as common ('c'), removed ('r'), or added ('+'). */
function classify(a: Token[], b: Token[], dp: Uint32Array): { aKind: string[]; bKind: string[] } {
  const m = a.length;
  const n = b.length;
  const aKind: string[] = new Array(m).fill('r');
  const bKind: string[] = new Array(n).fill('+');
  let i = m;
  let j = n;
  while (i > 0 && j > 0) {
    if (a[i - 1].text === b[j - 1].text) {
      aKind[i - 1] = 'c';
      bKind[j - 1] = 'c';
      i--;
      j--;
    } else {
      const up = dp[(i - 1) * (n + 1) + j];
      const left = dp[i * (n + 1) + (j - 1)];
      if (up >= left) {
        i--;
      } else {
        j--;
      }
    }
  }
  return { aKind, bKind };
}

/** Merge consecutive tokens that share a kind into character ranges.
 *  Adjacent ranges with the same kind collapse so the overlay isn't
 *  a swarm of one-character spans. */
function rangesFor(tokens: Token[], kinds: string[], target: string): WordDiffRange[] {
  const out: WordDiffRange[] = [];
  let cur: WordDiffRange | null = null;
  for (let i = 0; i < tokens.length; i++) {
    if (kinds[i] !== target) {
      if (cur) {
        out.push(cur);
        cur = null;
      }
      continue;
    }
    if (cur && cur.end === tokens[i].start) {
      cur.end = tokens[i].end;
    } else {
      if (cur) out.push(cur);
      cur = { start: tokens[i].start, end: tokens[i].end };
    }
  }
  if (cur) out.push(cur);
  return out;
}

/** Compute the word-level diff between two strings. Skips when one of
 *  the sides is empty (nothing meaningful to overlay) or when the strings
 *  share less than 30% of their tokens with each other — past that
 *  threshold the result tends to look like noise rather than a refined
 *  diff. */
export function diffLines(removeLine: string, addLine: string): LineWordDiff | null {
  if (removeLine.length === 0 || addLine.length === 0) return null;
  if (removeLine === addLine) {
    return { removed: [], added: [] };
  }
  const a = tokenize(removeLine);
  const b = tokenize(addLine);
  if (a.length === 0 || b.length === 0) return null;
  const dp = lcsTable(a, b);
  const lcs = dp[(a.length) * (b.length + 1) + b.length];
  const shorter = a.length < b.length ? a.length : b.length;
  if (lcs * 10 < shorter * 3) return null;
  const { aKind, bKind } = classify(a, b, dp);
  return {
    removed: rangesFor(a, aKind, 'r'),
    added: rangesFor(b, bKind, '+'),
  };
}

/** Wrap the syntax-highlighted HTML's text at the given character ranges
 *  with `<span class="{className}">` so the renderer's CSS can tint the
 *  changed words on top of the existing color spans. Uses the browser's
 *  DOM to handle entity decoding, span splitting, and re-serialization
 *  so we don't have to write an HTML parser by hand.
 *
 *  Originally hardcoded a `wd-` prefix for word-diff classes; now
 *  takes the full class string so callers can layer other overlays
 *  (e.g. intra-line comment column anchors → `'column-anchor'`)
 *  through the same DOM-walk machinery. */
export function wrapRanges(html: string, ranges: WordDiffRange[], className: string): string {
  if (ranges.length === 0) return html;
  if (typeof document === 'undefined') return html; // SSR safety; we don't render diffs server-side anyway
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  const root = tpl.content;

  // Walk text nodes, tracking absolute offset, and split + wrap when a
  // range straddles or sits inside a node. Ranges are pre-sorted; we
  // consume them in order.
  const sorted = ranges.slice().sort((x, y) => x.start - y.start);
  let rIdx = 0;
  let offset = 0;

  function walk(node: Node) {
    if (rIdx >= sorted.length) return;
    if (node.nodeType === Node.TEXT_NODE) {
      let text = (node as Text).data;
      let localStart = 0;
      while (rIdx < sorted.length) {
        const r = sorted[rIdx];
        const nodeEnd = offset + text.length;
        if (r.start >= nodeEnd) {
          // This range starts past this text node; move on.
          break;
        }
        // Range overlaps this text node somewhere.
        const sLocal = Math.max(0, r.start - offset);
        const eLocal = Math.min(text.length, r.end - offset);
        if (eLocal <= sLocal) {
          // Zero-width overlap (start lands at the node boundary).
          rIdx++;
          continue;
        }
        // Split the text node so the overlap becomes a separate Text
        // we can wrap. Carve [sLocal, eLocal) out of `text`.
        const before = text.slice(localStart, sLocal);
        const middle = text.slice(sLocal, eLocal);
        const after = text.slice(eLocal);
        const parent = node.parentNode!;
        if (before.length > 0) {
          parent.insertBefore(document.createTextNode(before), node);
        }
        const span = document.createElement('span');
        span.className = className;
        span.appendChild(document.createTextNode(middle));
        parent.insertBefore(span, node);
        if (after.length === 0) {
          parent.removeChild(node);
        } else {
          (node as Text).data = after;
          offset += sLocal + middle.length;
          text = after;
          localStart = 0;
          // The remainder of this node still needs to be checked against
          // later ranges, so don't advance rIdx unconditionally.
          if (r.end <= offset) {
            rIdx++;
            continue;
          }
          // Otherwise the range extends past this remainder; advance
          // when we leave the node.
          break;
        }
        offset += sLocal + middle.length;
        if (r.end > offset) {
          // Range extends into following nodes — leave rIdx unchanged.
          return;
        }
        rIdx++;
        // Move on to next range against subsequent siblings.
        return;
      }
      offset += text.length - localStart;
    } else {
      // Element or document fragment: descend, but iterate over a static
      // child snapshot since we mutate as we go.
      const children = Array.from(node.childNodes);
      for (const c of children) walk(c);
    }
  }

  walk(root);
  return (root as unknown as { firstChild: Element | null }).firstChild
    ? Array.from(root.childNodes)
        .map((n) =>
          n.nodeType === Node.TEXT_NODE
            ? escapeHtmlText((n as Text).data)
            : (n as Element).outerHTML,
        )
        .join('')
    : '';
}

function escapeHtmlText(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

interface LineLike {
  origin: 'removed' | 'added' | 'context';
  content: string;
}

/** Annotation for one hunk line: which side it's on plus the
 *  character ranges of changed words. */
export interface HunkLineWordDiff {
  kind: 'removed' | 'added';
  ranges: WordDiffRange[];
}

/** Walk a hunk's lines and compute per-line word-diff annotations for
 *  every paired remove/add block. Uses `alignBlock` to find the best
 *  content-similarity pairing rather than strict-index zipping, so
 *  word-diff highlights cover uneven blocks (3 removes / 4 adds, etc.)
 *  too — previously those blocks bailed out entirely and rendered as
 *  plain line-level highlights.
 *
 *  Returns a map keyed by the hunk-line index. Lines that don't
 *  pair (one-sided changes within the block, sub-threshold
 *  similarity) are absent; the caller falls back to the plain
 *  line-level highlight for those. */
export function computeHunkWordDiff(lines: LineLike[]): Map<number, HunkLineWordDiff> {
  const out = new Map<number, HunkLineWordDiff>();
  let i = 0;
  while (i < lines.length) {
    if (lines[i].origin !== 'removed') {
      i++;
      continue;
    }
    const removeStart = i;
    while (i < lines.length && lines[i].origin === 'removed') i++;
    const removeEnd = i;
    const addStart = i;
    while (i < lines.length && lines[i].origin === 'added') i++;
    const addEnd = i;
    const removeCount = removeEnd - removeStart;
    const addCount = addEnd - addStart;
    if (removeCount === 0 || addCount === 0) continue;
    const removeTexts = Array.from({ length: removeCount }, (_, k) =>
      lines[removeStart + k].content.replace(/\n$/, ''),
    );
    const addTexts = Array.from({ length: addCount }, (_, k) =>
      lines[addStart + k].content.replace(/\n$/, ''),
    );
    const alignment = alignBlock(removeTexts, addTexts);
    for (const { removeIndex, addIndex } of alignment.pairs) {
      const rIdx = removeStart + removeIndex;
      const aIdx = addStart + addIndex;
      const d = diffLines(removeTexts[removeIndex], addTexts[addIndex]);
      if (!d) continue;
      if (d.removed.length > 0) out.set(rIdx, { kind: 'removed', ranges: d.removed });
      if (d.added.length > 0) out.set(aIdx, { kind: 'added', ranges: d.added });
    }
  }
  return out;
}
