// Mirror of kata-core types. Hand-kept in sync with crates/kata-core.

export type ChangeId = string;
export type CommitId = string;
export type ReviewId = string;
export type SessionId = string;
export type CommentId = string;
export type ResponseId = string;
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

export interface CommitInfo {
  change_id: ChangeId;
  commit_id: CommitId;
  author_email: string;
  author_timestamp: string;
  description_first_line: string;
  /** Full commit description; may contain newlines or be empty. */
  description: string;
  /** Files this commit modified, added, deleted, or renamed (parent..@). */
  changed_files: string[];
}

export interface LineRange {
  start: number;
  end: number;
}

export type Side = 'base' | 'tip';
export type Flag = 'must-do' | 'suggestion' | 'other';
export type SessionStatus = 'draft' | 'published' | 'discarded';
export type ResolutionAction = 'comment' | 'resolve' | 'unresolve' | 'wont-fix';

export type FileStatusKind = 'added' | 'deleted' | 'modified' | 'renamed';
export type LineOrigin = 'context' | 'added' | 'removed';

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
  flag: Flag;
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

export interface Hunk {
  base_range?: LineRange;
  tip_range?: LineRange;
  lines: HunkLine[];
}

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
  commits: CommitInfo[];
  comments: CommentView[];
  responses: ResponseView[];
  drafts: DraftsView;
  /** True when re-resolving the manifest's revset would advance the
   *  current patchset. Used to gate the "Refresh" button. */
  is_stale: boolean;
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
  flag: Flag;
  body?: string;
}

export interface DraftResponseInput {
  in_reply_to: CommentId;
  action: ResolutionAction;
  body?: string;
}

export interface WhoAmI {
  author: Author;
}

/** What level of comment the composer is targeting. Line targets carry
 *  an inclusive `startLine..endLine` so multi-line selections work too.
 *  When `editing` is present the composer is editing an existing draft
 *  rather than creating a new one — submit goes via PUT and the anchor
 *  is the existing comment's anchor (kept verbatim). */
export type ComposerTarget = (
  | { kind: 'line'; file: string; side: Side; startLine: number; endLine: number }
  | { kind: 'file'; file: string }
  | { kind: 'review' }
) & {
  editing?: { commentId: string; body: string; flag: Flag };
};
