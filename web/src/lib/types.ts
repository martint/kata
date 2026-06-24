// Mirror of kata-core types. Hand-kept in sync with crates/kata-core.

export type ChangeId = string;
export type CommitId = string;
export type ReviewId = string;
export type SessionId = string;
export type CommentId = string;
export type ResponseId = string;
export type AnnotationId = string;
export type Author = string;
export type RevSet = string;
export type RepoId = string;

export interface RepoSummary {
  name: string;
  repo_id: RepoId;
  canonical_path: string;
}

export interface Bookmark {
  name: string;
  change_id: ChangeId;
  commit_id: CommitId;
  /** Author timestamp of the bookmark's commit (ISO 8601 with timezone, or
   *  empty if unknown). Used to sort branches "most recently updated first"
   *  on the create-review screen. */
  commit_timestamp: string;
}

/** One file touched by a commit, with its line-count summary against
 *  the commit's first parent. The shape mirrors what `read_commit_diff`
 *  reports for the same commit — same imara-diff histogram pass under
 *  the hood — so a reviewer's triage signal ("commit 2 is +1/-1") and
 *  the actual per-commit diff always agree. */
export interface ChangedFile {
  path: string;
  added: number;
  removed: number;
  binary?: boolean;
}

export interface CommitInfo {
  change_id: ChangeId;
  commit_id: CommitId;
  author_email: string;
  author_timestamp: string;
  description_first_line: string;
  /** Full commit description; may contain newlines or be empty. */
  description: string;
  /** Files this commit modified, added, deleted, or renamed (parent..@),
   *  with per-file +/- counts. */
  changed_files: ChangedFile[];
  /** Paths whose content at this commit is a conflict (jj keeps
   *  conflicts as live tree values). Empty for clean commits; the
   *  UI uses this to surface a ⚠ badge in the commits panel. */
  conflict_paths?: string[];
}

export interface LineRange {
  start: number;
  end: number;
}

// ---- Repository browser (graph log) ------------------------------------

export interface LogCoord {
  col: number;
  row: number;
}

/** Edge segment in the log graph. The renderer picks a path shape
 *  based on the `kind` tag. */
export type LogLine =
  | { kind: 'to-node'; source: LogCoord; target: LogCoord }
  | { kind: 'to-intersection'; source: LogCoord; target: LogCoord }
  | {
      kind: 'from-node';
      source: LogCoord;
      target: LogCoord;
      /** Column the rescue curve runs straight down before bending
       *  into target. Absent → no intermediate vertical. */
      via?: number;
    }
  | { kind: 'to-missing'; source: LogCoord; target: LogCoord };

export interface LogRow {
  commit: CommitInfo;
  /** Position of the node's circle. */
  location: LogCoord;
  /** Effective graph width at this row, in columns. The SVG renderer
   *  uses this to indent the text portion so neighbouring rows in a
   *  graph-connected run line up. */
  padding: number;
  lines: LogLine[];
  /** Bookmarks pointing at this commit's commit_id. */
  bookmarks?: string[];
  /** True iff this commit is the workspace's `@`. */
  is_working_copy?: boolean;
  /** True iff this commit is immutable — an ancestor of
   *  `immutable_heads()` (`trunk()` by default). */
  immutable?: boolean;
}

export interface LogPage {
  rows: LogRow[];
  /** True when the layout walk hit its row cap before exhausting
   *  the revset. */
  has_more: boolean;
}

export type Side = 'base' | 'tip';
export type Flag = 'must-do' | 'suggestion' | 'question';
export type SessionStatus = 'draft' | 'published' | 'discarded';
export type ResolutionAction = 'comment' | 'resolve' | 'unresolve' | 'wont-fix';

export type FileStatusKind = 'added' | 'deleted' | 'modified' | 'renamed';
export type LineOrigin = 'context' | 'added' | 'removed';

/** UTF-16 character range within a single line, half-open: `[start, end)`.
 *  Storage and the backend keep these as UTF-16 offsets because the
 *  browser's drag-to-select arithmetic (`Range.startOffset`,
 *  `String.length`) produces them natively. */
export interface ColumnRange {
  start: number;
  end: number;
}

export interface Comment {
  schema_version: number;
  comment_id: CommentId;
  session_id: SessionId;
  review_id: ReviewId;
  author: Author;
  created_at: string;
  /** Patchset the comment was written against. */
  patchset: number;
  anchor_change_id: ChangeId;
  anchor_commit_id: CommitId;
  file?: string;
  side?: Side;
  lines?: LineRange;
  /** Optional intra-line character range. Only set when `lines` is a
   *  single line; the renderer falls back to a line-level highlight
   *  when the line anchor is Drifted or Outdated. */
  columns?: ColumnRange;
  /** True when the comment is about the whole review rather than a
   *  specific commit. UI groups these under the "All commits" row.
   *  Mutually exclusive with `file`/`lines`. */
  review_wide?: boolean;
  flag: Flag;
  body: string;
  /** Identity of the original author when this comment was imported
   *  from a non-kata source (currently only GitHub PRs). `undefined`
   *  for native kata-authored comments — the UI then renders the
   *  structural `author` instead. */
  external_author?: ExternalAuthor;
}

export interface ExternalAuthor {
  /** Source identifier. `"github"` today. */
  source: string;
  login: string;
  /** Stable numeric identity on the source. */
  id: number;
  avatar_url?: string;
  /** Profile URL on the source. */
  html_url?: string;
}

/** Author-written annotation attached to a code region (or to the
 *  whole file / whole review). Annotations are one-way context — only
 *  `manifest.created_by` can author them, reviewers can read but not
 *  reply, no resolution state. */
export interface Annotation {
  schema_version: number;
  annotation_id: AnnotationId;
  review_id: ReviewId;
  author: Author;
  created_at: string;
  /** Last edit timestamp; equals `created_at` for unedited annotations. */
  updated_at: string;
  patchset: number;
  anchor_change_id: ChangeId;
  anchor_commit_id: CommitId;
  file?: string;
  side?: Side;
  lines?: LineRange;
  body: string;
}

export interface Response {
  schema_version: number;
  response_id: ResponseId;
  in_reply_to: CommentId;
  session_id: SessionId;
  author: Author;
  created_at: string;
  action: ResolutionAction;
  body: string;
}

export interface Session {
  schema_version: number;
  session_id: SessionId;
  review_id: ReviewId;
  author: Author;
  status: SessionStatus;
  created_at: string;
  published_at?: string;
}

export interface Patchset {
  n: number;
  base_change: ChangeId;
  base_commit: CommitId;
  tip_change: ChangeId;
  tip_commit: CommitId;
  recorded_at: string;
  /** Previous patchset whose tip is an ancestor of this one's tip; `null`
   *  when this patchset is on a disjoint branch from the previous round. */
  parent_patchset?: number | null;
}

export interface ReviewManifest {
  schema_version: number;
  /** Opaque internal identifier — UUID v7 for reviews created since
   *  the per-repo `number` rollout. Never shown to the user. */
  review_id: ReviewId;
  /** Per-repo monotonic number — what URLs and breadcrumbs use. */
  number: number;
  /** Human-readable name. Defaults to the bookmark slug at create
   *  time. Editable later; never affects URLs. */
  name: string;
  revset: RevSet;
  created_at: string;
  created_by: Author;
  bookmark?: string;
  /** Author-written markdown summary. Only the `created_by` author can
   *  set or update it. Absent on manifests that predate the feature. */
  summary?: string;
  patchsets: Patchset[];
  current_patchset: number;
  /** ISO-8601 timestamp of the most recent archive transition. Absent
   *  on active reviews. Only the creator may toggle it. Archived
   *  reviews are hidden from the home screen by default and reject new
   *  draft sessions. */
  archived_at?: string;
  /** GitHub PR this review is bound to, when created via
   *  `/api/github/import`. Drives the "Publish to GitHub" UI
   *  affordance and links back to the source PR. `undefined` for
   *  native kata reviews. */
  github_pr?: GithubPr;
}

export interface GithubPr {
  owner: string;
  repo: string;
  number: number;
  html_url: string;
  original_head_sha: string;
  original_base_sha: string;
}

/** Returned by `/api/github/status`. Drives whether the home
 *  screen offers the GitHub import card. */
export interface GithubStatus {
  connected: boolean;
  github_login?: string;
  /** Human-readable explanation when `connected` is false —
   *  distinguishes "install gh" from "run `gh auth login`". */
  error?: string;
}

export interface ReviewSummary {
  manifest: ReviewManifest;
  session_count: number;
  published_comment_count: number;
}

export interface HunkLine {
  origin: LineOrigin;
  base_line?: number;
  tip_line?: number;
  content: string;
}

/** A region of a file diff. The `kind` discriminator tells regular
 *  hunks (the historical shape — contiguous slice of changed +
 *  context lines) apart from conflict hunks (a structured view of
 *  the multiple sides of a jj conflict). The frontend pattern-matches
 *  on `kind` before reaching for variant-specific fields. */
export type Hunk = RegularHunk | ConflictHunk;

export interface RegularHunk {
  kind: 'regular';
  base_range?: LineRange;
  tip_range?: LineRange;
  lines: HunkLine[];
}

/** A conflict region as jj keeps it. Each *term* is one component
 *  of the merge: bases (the merge ancestors, from jj's `removes()`)
 *  followed by sides (the conflicting versions, from `adds()`).
 *  Labels are derived from parent commits when the system can
 *  correlate them (otherwise `Base` / `Side N`).
 *
 *  Side terms carry a per-line diff against the *first* base term,
 *  so the renderer can show what each side added or removed instead
 *  of stacking disconnected full-file blocks. The base term itself
 *  is rendered as plain Context content (it's the reference). */
export interface ConflictHunk {
  kind: 'conflict';
  terms: ConflictTerm[];
}

export interface ConflictTerm {
  label: string;
  kind: ConflictTermKind;
  /** Lines of this term, with origin tags relative to the first
   *  Base term in the enclosing `ConflictHunk`. For a `base` term
   *  all origins are `context`; for a `side` term they're a per-
   *  line diff against the base. */
  lines: HunkLine[];
}

export type ConflictTermKind = 'base' | 'side';

export interface FileChange {
  path: string;
  status: FileStatusKind;
  old_path?: string;
  hunks?: Hunk[];
  binary: boolean;
  /** Added line count. Always populated by the server (even when hunks
   *  are still lazy-loading) so the file tree's +/- can render before
   *  the per-file diff fetch resolves. Zero for binary files. */
  added: number;
  /** Removed line count. See [[added]]. */
  removed: number;
}

/** Result of fetching one commit's diff. Carries the endpoints' change
 *  ids alongside the file list so the UI can scope file reads, syntax
 *  highlighting, and new-comment anchoring to the clicked commit (not
 *  the whole-review patchset's tip, which can sit at completely
 *  different line numbers when later commits touch the same file). */
export interface CommitDiffView {
  base_change: ChangeId;
  base_commit: CommitId;
  tip_change: ChangeId;
  tip_commit: CommitId;
  files: FileChange[];
}

export interface Diff {
  base: CommitId;
  tip: CommitId;
  files: FileChange[];
}

// ---- patchset-compare v2 ------------------------------------------------

/** How a single change_id relates across two patchsets being compared. */
export type ChangeStatus =
  | 'same'
  | 'changed'
  | 'added-in-to'
  | 'removed-from-from';

export interface PatchsetPair {
  change_id: ChangeId;
  status: ChangeStatus;
  from_commit?: CommitId;
  to_commit?: CommitId;
  from_description?: string;
  to_description?: string;
  /** Parent of the present-side commit, populated by the backend for
   *  one-sided pairs (`added-in-to` / `removed-from-from`). Lets the
   *  UI render `parent..commit` for those rows. Absent on
   *  `same`/`changed` (no parent needed) and on `added`/`removed`
   *  where parent resolution failed (row falls back to inert). */
  parent_commit?: CommitId;
  /** Pre-computed diff counts for the row's effective endpoint pair
   *  (interdiff for `changed`; `parent..commit` for added/removed).
   *  Renders as a "3 files +7 −15" chip next to the description.
   *  Absent for `same` and when the count fetch failed. */
  diff_counts?: PairDiffCounts;
}

export interface PairDiffCounts {
  file_count: number;
  added: number;
  removed: number;
}

export interface PatchsetEndpoints {
  n: number;
  base_commit: CommitId;
  tip_commit: CommitId;
}

export interface PatchsetCompareView {
  from: PatchsetEndpoints;
  to: PatchsetEndpoints;
  cumulative: Diff;
  pairs: PatchsetPair[];
  compare_base_mismatch: boolean;
}

/** Result of `/api/repos/<repo>/diff?from=<a>&to=<b>[&path=<p>]`. The
 *  `kind` discriminator mirrors the Rust enum's serde tag. */
export type DiffCommitsResult =
  | ({ kind: 'diff' } & Diff)
  | ({ kind: 'file' } & FileChange);

export type AnchorView =
  | { kind: 'valid' }
  | { kind: 'moved'; new_lines: LineRange }
  | { kind: 'drifted'; new_lines: LineRange; similarity: number }
  | { kind: 'outdated'; original_content: string };

/** Comment with anchor resolution + draft flag. The server merges Comment's
 *  fields in flat via `#[serde(flatten)]`. */
export type CommentView = Comment & {
  anchor: AnchorView;
  draft: boolean;
};

/** Response with a draft flag (flattened from the Rust side). */
export type ResponseView = Response & {
  draft: boolean;
};

/** Annotation with anchor resolution. No `draft` flag — annotations
 *  publish immediately, there's no draft / batch-publish cycle. */
export type AnnotationView = Annotation & {
  anchor: AnchorView;
};

/** UI-side resolution state derived from a comment's responses. */
export type ResolutionState = 'open' | 'resolved' | 'wont-fix';

export interface DraftsView {
  session?: Session;
  comments: CommentView[];
  responses: ResponseView[];
}

export interface ReviewView {
  manifest: ReviewManifest;
  diff: Diff;
  /** True when the diff/commit list couldn't be computed — typically a
   *  pinned base/tip commit was garbage-collected out of the repo. The
   *  review still loads (chrome, comments, annotations); `diff` and
   *  `commits` come back empty and the UI shows an explanatory banner
   *  instead of bouncing to the home screen. Absent in the normal case. */
  diff_unavailable?: boolean;
  commits: CommitInfo[];
  comments: CommentView[];
  responses: ResponseView[];
  /** Author-attached annotations across the review. Absent or empty
   *  on reviews that haven't used the feature. */
  annotations?: AnnotationView[];
  drafts: DraftsView;
  /** True when re-resolving the manifest's revset would advance the
   *  current patchset. Used to gate the "Refresh" button. */
  is_stale: boolean;
  /** Present when the live revset failed to resolve (e.g. divergent
   *  change ID). Carries the cleaned jj message plus, for divergent
   *  changes, the commit IDs the reader needs to disambiguate. UI
   *  renders a warning banner. */
  revset_error?: RevsetError;
  /** Review-relevant activity that landed between the viewer's
   *  previous open and the current one. Absent on first ever open
   *  (no baseline) and when no qualifying activity happened. The
   *  banner renders as e.g. "Since you were here: 2 new comments,
   *  1 new patchset". */
  unread?: UnreadSummary;
  /** Wall-clock timestamp the viewer last opened this review at.
   *  Used to flag comments with responses newer than this as having
   *  unread replies. Absent on the viewer's first ever open. */
  last_visit_at?: string;
}

export interface UnreadSummary {
  /** Patchsets recorded since the previous visit. */
  new_patchsets?: number;
  /** Comments by other authors published since the previous visit. */
  new_comments?: number;
  /** Replies by other authors created since the previous visit. */
  new_replies?: number;
  /** Annotations by other authors created since the previous visit. */
  new_annotations?: number;
}

export interface RevsetError {
  /** jj's stderr with the `Error: ` framing stripped. */
  message: string;
  /** Candidates for `jj abandon` when the failure is a divergent
   *  change ID. Carries enough metadata (timestamp + description)
   *  for the reader to tell the copies apart. Absent / empty for
   *  other errors. */
  divergent_commits?: DivergentCommit[];
}

export interface DivergentCommit {
  commit_id: CommitId;
  /** ISO 8601. */
  author_timestamp: string;
  description_first_line: string;
}

export interface CreateReviewParams {
  /** Human label; defaults to the bookmark name. Server generates
   *  the internal `review_id` and assigns the per-repo `number`. */
  name: string;
  revset: RevSet;
  bookmark?: string;
  created_by: Author;
  /** Optional markdown summary shown at the top of the review. */
  summary?: string;
}

export interface DraftCommentInput {
  anchor_change_id: ChangeId;
  anchor_commit_id: CommitId;
  file?: string;
  side?: Side;
  lines?: LineRange;
  /** Optional intra-line character range. Required to be a single-line
   *  range (start == end) when set; server rejects otherwise. */
  columns?: ColumnRange;
  review_wide?: boolean;
  flag: Flag;
  body?: string;
}

export interface DraftResponseInput {
  in_reply_to: CommentId;
  action: ResolutionAction;
  body?: string;
}

/** Create/update body for annotations. Same anchor fields as a
 *  comment, minus `flag` (annotations have no severity) and
 *  `review_wide` (a review-wide annotation is just one with no
 *  `file`). */
export interface AnnotationInput {
  anchor_change_id: ChangeId;
  anchor_commit_id: CommitId;
  file?: string;
  side?: Side;
  lines?: LineRange;
  body?: string;
}

export interface WhoAmI {
  author: Author;
  /** True when the server resolved this caller as a global admin
   *  (creator-equivalent on every review). Defaults to `false` for
   *  older servers that don't send the field. */
  is_admin?: boolean;
}

/** What level of comment the composer is targeting. Line targets carry
 *  an inclusive `startLine..endLine` so multi-line selections work too.
 *  `commit` targets carry the change_id and commit_id of the commit the
 *  comment is about, so the anchor is fixed to that specific change
 *  regardless of where the review's tip is now. When `editing` is
 *  present the composer is editing an existing draft rather than
 *  creating a new one — submit goes via PUT and the anchor is the
 *  existing comment's anchor (kept verbatim). */
export type ComposerTarget = (
  | {
      kind: 'line';
      file: string;
      side: Side;
      startLine: number;
      endLine: number;
      /** Optional intra-line character range (UTF-16, `[start, end)`).
       *  Only carried when the user drag-selected text within a
       *  single line — gets persisted on the resulting comment as
       *  `columns`. */
      columns?: ColumnRange;
    }
  | { kind: 'file'; file: string }
  | { kind: 'commit'; change_id: ChangeId; commit_id: CommitId }
  | { kind: 'review' }
) & {
  editing?: { commentId: string; body: string; flag: Flag };
};
