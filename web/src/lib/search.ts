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
//! Scope of v2:
//! - File paths (so "foo.ts" finds files literally named that).
//! - Diff lines (regular hunks only — conflict regions are
//!   structurally different and rarer; skipped for now).
//! - Commit messages (one-line descriptions in the commits panel).
//! - Comment bodies (published + draft).
//! - Response bodies (replies + resolution markers).
//! - Annotation bodies.
//! - Review name + summary.
//! - Case-insensitive substring match. No regex.
//!
//! Ordering: per-file matches first, in `files` order — file-path
//! match (one per matching file) → diff lines → comments / responses
//! / annotations anchored to that file. Then the cross-file buckets:
//! commits, then review-wide comments / annotations, then the
//! review-metadata match (name / summary) so a query that hits both
//! a file and the summary lands the reader inside the diff first
//! (the more usual destination) and surfaces the metadata as a
//! follow-up.
//!
//! Out of scope (deferred): regex, case-sensitive toggle, search-
//! inside-conflict-region, file-tree match-count badges.

import type {
  AnnotationView,
  ChangeId,
  CommentView,
  FileChange,
  ResponseView,
} from './types';

/** Just the bits of `CommitInfo` the search needs. Decoupled from
 *  the full structure so callers don't have to construct the
 *  per-commit `changed_files` / `conflict_paths` fields for the
 *  search source. */
export interface CommitInfoLite {
  change_id: ChangeId;
  description_first_line: string;
}

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
      kind: 'response';
      response_id: string;
      /** The comment this response replies to — drives the
       *  scroll-to-match: jumping to a response lands on its parent
       *  comment's anchor, where the response is rendered inline. */
      in_reply_to: string;
      /** File / line of the parent comment, mirrored here so the
       *  ordering pass can interleave response matches with their
       *  parent comment's diff bucket without an extra lookup. */
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
    }
  | {
      kind: 'file';
      /** Path of the file whose name matched. The renderer jumps to
       *  the file-tree row + scrolls the file's slot into view. */
      file: string;
      /** UTF-16 offsets into `snippet` (== `file`). */
      matchStart: number;
      matchEnd: number;
      snippet: string;
    }
  | {
      kind: 'commit';
      change_id: string;
      /** Commit's one-line description — the haystack the match was
       *  found in. */
      snippet: string;
      matchStart: number;
      matchEnd: number;
    }
  | {
      kind: 'review-meta';
      /** Which review-level field matched — drives the icon /
       *  destination on click. */
      field: 'name' | 'summary';
      snippet: string;
      matchStart: number;
      matchEnd: number;
    };

export interface SearchSource {
  /** The review's full file list. Files whose `hunks` is undefined
   *  are skipped at the diff-line scan — the caller is responsible
   *  for force-loading before searching if it wants diff matches
   *  for those files included. File-path matches happen regardless
   *  (the path is metadata and always present). */
  files: readonly FileChange[];
  comments: readonly CommentView[];
  /** Responses (replies + resolution markers), published + draft.
   *  Resolution-only responses (empty body) contribute nothing to
   *  the search source — they're filtered out implicitly by the
   *  body-length check inside the scan. */
  responses: readonly ResponseView[];
  annotations: readonly AnnotationView[];
  /** Per-commit one-line descriptions from the commits panel.
   *  Searching for "fix typo" should land the reader on that
   *  commit row even when the diff hunks don't contain the
   *  string. */
  commits: readonly CommitInfoLite[];
  /** Review-level metadata. The review's name shows in the title
   *  chrome; the summary lives in the prose panel above the file
   *  tree. Both are reasonable things for a reader to grep for. */
  reviewName: string;
  reviewSummary?: string | null;
}

/** Find every occurrence of `query` across the review's content —
 *  see the module header for the full list of sources. Case-
 *  insensitive. Returns matches in reading order (see the module
 *  header for the ordering contract).
 *
 *  An empty / whitespace-only query returns an empty array.
 */
export function searchReview(
  query: string,
  source: SearchSource,
): SearchMatch[] {
  const q = query.toLowerCase();
  if (q.length === 0) return [];

  // Bucket comments / responses / annotations by their anchored
  // file path so we can interleave them with their file's diff
  // matches below. The null bucket holds review-wide / file-less
  // entries — those land in the cross-file section so the reader
  // walks them last.
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
  // Responses bucket by the file of their parent comment, so a
  // reply's match lands in the same per-file group as the comment
  // it threads under. Responses whose parent we can't find (rare
  // — would mean a draft response orphaned from its comment) get
  // dropped from the index rather than silently surfacing in the
  // wrong place.
  const commentById = new Map<string, CommentView>();
  for (const c of source.comments) commentById.set(c.comment_id, c);
  const responsesByFile = new Map<string | null, ResponseView[]>();
  for (const r of source.responses) {
    if (r.body.length === 0) continue; // pure resolution markers — nothing to search
    const parent = commentById.get(r.in_reply_to);
    if (!parent) continue;
    const key = parent.file ?? null;
    const list = responsesByFile.get(key) ?? [];
    list.push(r);
    responsesByFile.set(key, list);
  }

  const out: SearchMatch[] = [];

  for (const file of source.files) {
    // File-path match comes first inside the file's bucket so a
    // user searching for the filename lands on the file header
    // before being walked through its diff content.
    pushPathMatches(out, q, file.path);

    // Diff lines.
    if (file.hunks) {
      for (const hunk of file.hunks) {
        // Conflict hunks ship their content as a `Vec<ConflictTerm>`
        // rather than the regular base/tip line list. Searching them
        // requires a different shape (per-term label + diff-vs-base
        // lines); out of v2 scope.
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

    // Comment + response + annotation matches anchored to this
    // file, in line-order so the reader walks them top-to-bottom
    // along with the diff matches above. The "anchor" we sort on
    // is just the line number — partial-selection / multi-line
    // nuance doesn't matter for the linear next/prev walk.
    const fileComments = commentsByFile.get(file.path) ?? [];
    const fileResponses = responsesByFile.get(file.path) ?? [];
    const fileAnnotations = annotationsByFile.get(file.path) ?? [];
    pushBodyMatches(out, q, fileComments, fileResponses, fileAnnotations);
  }

  // Cross-file buckets.

  // Commits — search their one-line descriptions. Order matches the
  // commits panel's natural order (oldest-first), which is how the
  // service emits them.
  for (const c of source.commits) {
    pushTextMatches(q, c.description_first_line, (mi, end) => {
      out.push({
        kind: 'commit',
        change_id: c.change_id,
        snippet: c.description_first_line,
        matchStart: mi,
        matchEnd: end,
      });
    });
  }

  // Review-wide and any leftover file-less comment / response /
  // annotation items.
  pushBodyMatches(
    out,
    q,
    commentsByFile.get(null) ?? [],
    responsesByFile.get(null) ?? [],
    annotationsByFile.get(null) ?? [],
  );

  // Review name + summary land last — most readers searching are
  // looking for content inside the review, not the title; surface
  // those when nothing else matched but don't preempt diff hits.
  pushTextMatches(q, source.reviewName, (mi, end) => {
    out.push({
      kind: 'review-meta',
      field: 'name',
      snippet: source.reviewName,
      matchStart: mi,
      matchEnd: end,
    });
  });
  if (source.reviewSummary) {
    pushTextMatches(q, source.reviewSummary, (mi, end) => {
      out.push({
        kind: 'review-meta',
        field: 'summary',
        snippet: source.reviewSummary!,
        matchStart: mi,
        matchEnd: end,
      });
    });
  }

  return out;
}

/** CSS selector for the DOM element to scroll to for a match. Stays
 *  in sync with the data attributes the renderers apply:
 *  `data-side`+`data-line` on diff rows (scoped to the match's
 *  `.file-slot[data-file-path]`, because line numbers repeat across
 *  files — an unscoped `[data-side][data-line]` resolves to whichever
 *  file mounted first, sending the reader to a line that doesn't hold
 *  the match), `data-comment-id` in CommentThread, `data-annotation-id`
 *  on AnnotationBubble, `data-change-id` on the commit-row, and
 *  `data-file-path` on the FileSlot wrapper. A `response` reuses its
 *  parent comment's anchor (the reply renders inline beneath it).
 *  `review-meta` targets the top of the page, so it has no selector. */
export function selectorForMatch(m: SearchMatch): string | null {
  switch (m.kind) {
    case 'line':
      return `.file-slot[data-file-path="${CSS.escape(m.file)}"] [data-side="${m.side}"][data-line="${m.line}"]`;
    case 'comment':
      return `[data-comment-id="${CSS.escape(m.comment_id)}"]`;
    case 'response':
      return `[data-comment-id="${CSS.escape(m.in_reply_to)}"]`;
    case 'annotation':
      return `[data-annotation-id="${CSS.escape(m.annotation_id)}"]`;
    case 'file':
      return `.file-slot[data-file-path="${CSS.escape(m.file)}"]`;
    case 'commit':
      return `[data-change-id="${CSS.escape(m.change_id)}"]`;
    case 'review-meta':
      return null;
  }
}

/** Push a file-path match if `q` matches anywhere in `path`. One
 *  match per `(file, occurrence)` — typical paths contain the
 *  query at most once, but the multi-pass loop handles repeats
 *  cleanly (`src/foo/foo.ts` for `foo`). */
function pushPathMatches(out: SearchMatch[], q: string, path: string): void {
  pushTextMatches(q, path, (mi, end) => {
    out.push({
      kind: 'file',
      file: path,
      snippet: path,
      matchStart: mi,
      matchEnd: end,
    });
  });
}

/** Append comment + response + annotation matches for a single
 *  "bucket" (one file's anchored items, or the review-wide
 *  bucket) to `out`. Sort each list by line number so the reader
 *  walks them top-to-bottom. */
function pushBodyMatches(
  out: SearchMatch[],
  q: string,
  comments: readonly CommentView[],
  responses: readonly ResponseView[],
  annotations: readonly AnnotationView[],
): void {
  // Comments first, sorted by line number (no lines → 0 → first).
  const sortedComments = [...comments].sort(
    (a, b) => (a.lines?.start ?? 0) - (b.lines?.start ?? 0),
  );
  for (const c of sortedComments) {
    pushTextMatches(q, c.body, (mi, end) => {
      out.push({
        kind: 'comment',
        comment_id: c.comment_id,
        file: c.file ?? null,
        line: c.lines?.start ?? null,
        snippet: c.body,
        matchStart: mi,
        matchEnd: end,
      });
    });
  }
  // Responses, sorted by their `created_at` so older replies
  // surface first inside the bucket — closest to reading order
  // when a thread accumulates replies over time.
  const sortedResponses = [...responses].sort(
    (a, b) => a.created_at.localeCompare(b.created_at),
  );
  for (const r of sortedResponses) {
    const parent = comments.find((c) => c.comment_id === r.in_reply_to) ?? null;
    pushTextMatches(q, r.body, (mi, end) => {
      out.push({
        kind: 'response',
        response_id: r.response_id,
        in_reply_to: r.in_reply_to,
        file: parent?.file ?? null,
        line: parent?.lines?.start ?? null,
        snippet: r.body,
        matchStart: mi,
        matchEnd: end,
      });
    });
  }
  const sortedAnnotations = [...annotations].sort(
    (a, b) => (a.lines?.start ?? 0) - (b.lines?.start ?? 0),
  );
  for (const a of sortedAnnotations) {
    pushTextMatches(q, a.body, (mi, end) => {
      out.push({
        kind: 'annotation',
        annotation_id: a.annotation_id,
        file: a.file ?? null,
        line: a.lines?.start ?? null,
        snippet: a.body,
        matchStart: mi,
        matchEnd: end,
      });
    });
  }
}

/** Scan `haystack` for every occurrence of `q` (already lower-
 *  cased) and invoke `emit(matchStart, matchEnd)` for each. UTF-16
 *  offsets so the emit callback can plug straight into the
 *  `wrapRanges` injector. */
function pushTextMatches(
  q: string,
  haystack: string,
  emit: (matchStart: number, matchEnd: number) => void,
): void {
  if (haystack.length === 0) return;
  const lower = haystack.toLowerCase();
  for (let i = lower.indexOf(q); i !== -1; i = lower.indexOf(q, i + 1)) {
    emit(i, i + q.length);
  }
}
