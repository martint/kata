//! In-app search across a review's diff text, comment bodies, and
//! annotation bodies.
//!
//! Why this exists: browser find (Ctrl/Cmd+F) doesn't work usefully
//! against a Kata review. `FileSlot` virtualises files outside the
//! viewport — their hunks aren't in the DOM at all — and file-fold
//! collapses hide content even for the file the user is looking at.
//! So the native search reaches only a fraction of what's actually
//! in the review.
//!
//! This module is the data side: a pure function that scans an
//! in-memory representation and returns matches in reading order.
//! The UI layer (`ReviewSearch.svelte`) decides what to do with
//! them — render counters, walk prev/next, scroll-and-highlight.
//!
//! Scope of v1:
//! - Diff lines (regular hunks only — conflict regions are
//!   structurally different and rarer; skipped for now).
//! - Comment bodies (published + draft).
//! - Annotation bodies.
//! - Case-insensitive substring match. No regex.
//!
//! Out of scope (deferred): file-path filter, regex, case-sensitive
//! toggle, response-body matches, file-tree match-count badges.

import type {
  AnnotationView,
  CommentView,
  FileChange,
} from './types';

export type SearchMatch =
  | {
      kind: 'line';
      /** File the match lives in, used for ordering and for the
       *  scroll-to-match jump. */
      file: string;
      /** Side of the diff the match is on. Context lines exist on
       *  both sides; we emit `tip` for those (renderer highlights
       *  whichever cells render the line). Added → tip; removed →
       *  base. */
      side: 'base' | 'tip';
      /** 1-based line number on `side` — same number the rendered
       *  gutter shows. */
      line: number;
      /** The full line content (with the trailing newline stripped),
       *  used for the result snippet and for offset arithmetic
       *  against the rendered HTML. */
      snippet: string;
      /** Inclusive start, exclusive end. UTF-16 offsets — matches
       *  what `String.indexOf` / `String.length` produce, which is
       *  what the renderer's `<mark>` injector consumes. */
      matchStart: number;
      matchEnd: number;
    }
  | {
      kind: 'comment';
      comment_id: string;
      /** File the comment anchors to, or `null` for review-wide.
       *  Drives ordering — file-anchored matches sort with that
       *  file's diff matches; review-wide matches go last. */
      file: string | null;
      line: number | null;
      snippet: string;
      matchStart: number;
      matchEnd: number;
    }
  | {
      kind: 'annotation';
      annotation_id: string;
      file: string | null;
      line: number | null;
      snippet: string;
      matchStart: number;
      matchEnd: number;
    };

export interface SearchSource {
  /** The review's full file list. Files whose `hunks` is undefined
   *  are skipped — the caller is responsible for force-loading
   *  before searching if it wants those files included. */
  files: readonly FileChange[];
  comments: readonly CommentView[];
  annotations: readonly AnnotationView[];
}

/** Find every occurrence of `query` across the review's diff lines
 *  and comment / annotation bodies. Case-insensitive. Returns
 *  matches in reading order: per file (in `files` order), each
 *  file's diff line matches followed by its anchored
 *  comment / annotation matches; review-wide comments and
 *  annotations come last.
 *
 *  An empty / whitespace-only query returns an empty array.
 */
export function searchReview(
  query: string,
  source: SearchSource,
): SearchMatch[] {
  const q = query.toLowerCase();
  if (q.length === 0) return [];

  const fileOrder = new Map<string, number>();
  source.files.forEach((f, i) => fileOrder.set(f.path, i));

  // Bucket comments + annotations by their anchored file path so we
  // can interleave them with their file's diff matches below. The
  // null bucket holds review-wide / file-less entries — those land
  // at the end so the reader walks them last.
  const commentsByFile = new Map<string | null, CommentView[]>();
  for (const c of source.comments) {
    const key = c.file ?? null;
    const list = commentsByFile.get(key) ?? [];
    list.push(c);
    commentsByFile.set(key, list);
  }
  const annotationsByFile = new Map<string | null, AnnotationView[]>();
  for (const a of source.annotations) {
    const key = a.file ?? null;
    const list = annotationsByFile.get(key) ?? [];
    list.push(a);
    annotationsByFile.set(key, list);
  }

  const out: SearchMatch[] = [];

  for (const file of source.files) {
    // Diff lines first.
    if (file.hunks) {
      for (const hunk of file.hunks) {
        // Conflict hunks ship their content as a `Vec<ConflictTerm>`
        // rather than the regular base/tip line list. Searching them
        // requires a different shape (per-term label + diff-vs-base
        // lines); out of v1 scope.
        if (hunk.kind !== 'regular') continue;
        for (const line of hunk.lines) {
          const content = line.content.replace(/\n$/, '');
          if (content.length === 0) continue;
          const haystack = content.toLowerCase();
          // Side preference: context lines exist on both sides;
          // emit one match per occurrence, preferring `tip` (the
          // side the reader looks at by default). Added lines are
          // tip-only; removed lines are base-only.
          const side: 'base' | 'tip' =
            line.tip_line != null ? 'tip' : 'base';
          const lineNum =
            line.tip_line != null ? line.tip_line : line.base_line!;
          for (
            let i = haystack.indexOf(q);
            i !== -1;
            i = haystack.indexOf(q, i + 1)
          ) {
            out.push({
              kind: 'line',
              file: file.path,
              side,
              line: lineNum,
              snippet: content,
              matchStart: i,
              matchEnd: i + q.length,
            });
          }
        }
      }
    }

    // Comment + annotation matches anchored to this file, in
    // line-order so the reader walks them top-to-bottom along with
    // the diff matches above. The "anchor" we sort on is just the
    // line number — partial-selection / multi-line nuance doesn't
    // matter for the linear next/prev walk.
    const fileComments = commentsByFile.get(file.path) ?? [];
    const fileAnnotations = annotationsByFile.get(file.path) ?? [];
    pushBodyMatches(out, q, fileComments, fileAnnotations);
  }

  // Review-wide and any leftover file-less items.
  pushBodyMatches(
    out,
    q,
    commentsByFile.get(null) ?? [],
    annotationsByFile.get(null) ?? [],
  );

  return out;
}

/** Append comment + annotation matches for a single "bucket" (one
 *  file's anchored items, or the review-wide bucket) to `out`. The
 *  caller decides ordering across buckets; this helper just
 *  preserves line-then-comment order within one. */
function pushBodyMatches(
  out: SearchMatch[],
  q: string,
  comments: readonly CommentView[],
  annotations: readonly AnnotationView[],
): void {
  // Comments first, sorted by line number (no lines → 0 → first).
  const sortedComments = [...comments].sort(
    (a, b) => (a.lines?.start ?? 0) - (b.lines?.start ?? 0),
  );
  for (const c of sortedComments) {
    const haystack = c.body.toLowerCase();
    for (
      let i = haystack.indexOf(q);
      i !== -1;
      i = haystack.indexOf(q, i + 1)
    ) {
      out.push({
        kind: 'comment',
        comment_id: c.comment_id,
        file: c.file ?? null,
        line: c.lines?.start ?? null,
        snippet: c.body,
        matchStart: i,
        matchEnd: i + q.length,
      });
    }
  }
  const sortedAnnotations = [...annotations].sort(
    (a, b) => (a.lines?.start ?? 0) - (b.lines?.start ?? 0),
  );
  for (const a of sortedAnnotations) {
    const haystack = a.body.toLowerCase();
    for (
      let i = haystack.indexOf(q);
      i !== -1;
      i = haystack.indexOf(q, i + 1)
    ) {
      out.push({
        kind: 'annotation',
        annotation_id: a.annotation_id,
        file: a.file ?? null,
        line: a.lines?.start ?? null,
        snippet: a.body,
        matchStart: i,
        matchEnd: i + q.length,
      });
    }
  }
}
