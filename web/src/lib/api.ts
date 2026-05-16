import type {
  Bookmark,
  Comment,
  CommitDiffView,
  CreateReviewParams,
  DiffCommitsResult,
  DraftCommentInput,
  DraftResponseInput,
  FileChange,
  PatchsetCompareView,
  RepoSummary,
  Response as ReviewResponse,
  ReviewManifest,
  ReviewSummary,
  ReviewView,
  Session,
  WhoAmI,
} from './types';

export class ApiError extends Error {
  constructor(
    public status: number,
    public detail: string,
  ) {
    super(detail);
  }
}

async function fetchText(path: string): Promise<string> {
  const res = await fetch(path);
  if (!res.ok) {
    let detail = res.statusText;
    try {
      const json = (await res.json()) as { error?: string };
      detail = json.error ?? detail;
    } catch {
      // not JSON; keep statusText
    }
    throw new ApiError(res.status, detail);
  }
  return await res.text();
}

/** Cache of in-flight + resolved file-content reads keyed by
 *  `${commit}|${path}`. Commit IDs are immutable (jj/git), so a hit
 *  is always valid for the tab's lifetime. Storing the Promise (not
 *  the resolved string) dedupes concurrent fetches for the same
 *  key — multiple FileDiff mounts of the same file share one
 *  network round-trip. */
const readFileCache = new Map<string, Promise<string>>();

function cachedReadFile(repo: string, commit: string, path: string): Promise<string> {
  const key = `${commit}|${path}`;
  const hit = readFileCache.get(key);
  if (hit) return hit;
  const pending = fetchText(
    `${repoBase(repo)}/files?commit=${enc(commit)}&path=${enc(path)}`,
  ).catch((err) => {
    // Don't poison the cache with a failure — let the next caller retry.
    readFileCache.delete(key);
    throw err;
  });
  readFileCache.set(key, pending);
  return pending;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = { method };
  if (body !== undefined) {
    init.headers = { 'content-type': 'application/json' };
    init.body = JSON.stringify(body);
  }
  const res = await fetch(path, init);
  if (!res.ok) {
    let detail = res.statusText;
    try {
      const json = (await res.json()) as { error?: string };
      detail = json.error ?? detail;
    } catch {
      // body wasn't JSON; fall back to statusText
    }
    throw new ApiError(res.status, detail);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

const enc = encodeURIComponent;

const repoBase = (repo: string) => `/api/repos/${enc(repo)}`;

export const api = {
  whoami: () => request<WhoAmI>('GET', '/api/whoami'),
  listRepos: () => request<RepoSummary[]>('GET', '/api/repos'),

  listBookmarks: (repo: string) =>
    request<Bookmark[]>('GET', `${repoBase(repo)}/bookmarks`),

  /** Probe a revset against the jj backend to count its commits.
   *  Used by the new-review form to warn before submitting an empty
   *  or malformed revset. Throws `ApiError` on syntax / resolution
   *  errors — the form surfaces those inline. */
  previewRevset: (repo: string, expr: string) =>
    request<{ count: number }>(
      'GET',
      `${repoBase(repo)}/revset/preview?expr=${enc(expr)}`,
    ),

  listReviews: (repo: string) =>
    request<ReviewSummary[]>('GET', `${repoBase(repo)}/reviews`),
  createReview: (repo: string, params: CreateReviewParams) =>
    request<ReviewManifest>('POST', `${repoBase(repo)}/reviews`, params),
  /** Every review-scoped endpoint identifies the review by its
   *  per-repo `number` — what the URL bar shows. The internal
   *  `review_id` (UUID) is never exposed in API paths. */
  openReview: (
    repo: string,
    number: number,
    patchset?: number,
    compare?: number,
  ) => {
    const parts: string[] = [];
    if (patchset !== undefined) parts.push(`patchset=${patchset}`);
    if (compare !== undefined) parts.push(`compare=${compare}`);
    const qs = parts.length > 0 ? `?${parts.join('&')}` : '';
    return request<ReviewView>('GET', `${repoBase(repo)}/reviews/${number}${qs}`);
  },
  refreshReview: (repo: string, number: number, summary?: string) =>
    request<ReviewManifest>(
      'POST',
      `${repoBase(repo)}/reviews/${number}/refresh`,
      summary !== undefined ? { summary } : {},
    ),
  updateSummary: (repo: string, number: number, summary: string | null) =>
    request<ReviewManifest>(
      'PUT',
      `${repoBase(repo)}/reviews/${number}/summary`,
      { summary },
    ),
  /** Mark the review archived (creator-only). The returned manifest
   *  carries the new `archived_at`. Other tabs learn via the
   *  `review-updated` SSE event. */
  archiveReview: (repo: string, number: number) =>
    request<ReviewManifest>('POST', `${repoBase(repo)}/reviews/${number}/archive`),
  unarchiveReview: (repo: string, number: number) =>
    request<ReviewManifest>('DELETE', `${repoBase(repo)}/reviews/${number}/archive`),
  commitDiff: (repo: string, number: number, changeId: string) =>
    request<CommitDiffView>(
      'GET',
      `${repoBase(repo)}/reviews/${number}/commits/${enc(changeId)}/diff`,
    ),
  /** Hunks for one file in a review. `openReview` ships only the
   *  file list, then the UI calls this per FileSlot as files scroll
   *  into view. `ps` selects the patchset; omit for "latest".
   *  `compare`, when set, makes the response describe the
   *  patchset[compare] → patchset[ps] delta rather than base..tip
   *  (must match what the metadata response was fetched with). */
  fileDiff: (
    repo: string,
    number: number,
    path: string,
    ps?: number,
    compare?: number,
  ) => {
    const parts: string[] = [`path=${enc(path)}`];
    if (ps !== undefined) parts.push(`ps=${ps}`);
    if (compare !== undefined) parts.push(`compare=${compare}`);
    return request<FileChange>(
      'GET',
      `${repoBase(repo)}/reviews/${number}/file-diff?${parts.join('&')}`,
    );
  },
  readFile: cachedReadFile,

  /** Patchset-compare v2: cumulative diff metadata + per-change-id
   *  pair list for the (from, to) patchset pair. The per-commit
   *  interdiff *content* is not in this payload — call `diffCommits`
   *  with the pair's `from_commit` / `to_commit` to fetch that. */
  comparePatchsets: (repo: string, number: number, from: number, to: number) =>
    request<PatchsetCompareView>(
      'GET',
      `${repoBase(repo)}/reviews/${number}/compare?from=${from}&to=${to}`,
    ),

  /** Generic commit-pair diff. Omit `path` for file-level metadata;
   *  supply `path` for the hunks of that single file. Used by the
   *  per-commit view in compare mode to fetch interdiffs.
   *
   *  When `interdiff: true`, the backend runs a *rebase-based*
   *  interdiff instead of the literal diff(from, to): it rebases
   *  `from` onto `to`'s parent in-memory via jj-lib, then diffs the
   *  rebased tree against `to`. Use this for `changed` pair rows in
   *  patchset-compare so downstream-of-rewrite commits show only the
   *  delta they actually contribute, not the inherited cumulative
   *  tree difference. For added/removed pairs (`parent..commit`)
   *  leave it false — they're plain commit-own diffs. */
  diffCommits: (
    repo: string,
    from: string,
    to: string,
    path?: string,
    interdiff: boolean = false,
  ) => {
    const parts: string[] = [`from=${enc(from)}`, `to=${enc(to)}`];
    if (path !== undefined) parts.push(`path=${enc(path)}`);
    if (interdiff) parts.push('interdiff=true');
    return request<DiffCommitsResult>(
      'GET',
      `${repoBase(repo)}/diff?${parts.join('&')}`,
    );
  },

  startSession: (repo: string, number: number) =>
    request<Session>('POST', `${repoBase(repo)}/reviews/${number}/sessions`),
  publishSession: (repo: string, number: number, sid: string) =>
    request<void>(
      'POST',
      `${repoBase(repo)}/reviews/${number}/sessions/${enc(sid)}/publish`,
    ),
  discardSession: (repo: string, number: number, sid: string) =>
    request<void>(
      'POST',
      `${repoBase(repo)}/reviews/${number}/sessions/${enc(sid)}/discard`,
    ),

  createComment: (
    repo: string,
    number: number,
    sid: string,
    input: DraftCommentInput,
  ) =>
    request<Comment>(
      'POST',
      `${repoBase(repo)}/reviews/${number}/sessions/${enc(sid)}/comments`,
      input,
    ),
  updateComment: (
    repo: string,
    number: number,
    sid: string,
    cid: string,
    input: DraftCommentInput,
  ) =>
    request<Comment>(
      'PUT',
      `${repoBase(repo)}/reviews/${number}/sessions/${enc(sid)}/comments/${enc(cid)}`,
      input,
    ),
  deleteComment: (repo: string, number: number, sid: string, cid: string) =>
    request<void>(
      'DELETE',
      `${repoBase(repo)}/reviews/${number}/sessions/${enc(sid)}/comments/${enc(cid)}`,
    ),

  createResponse: (
    repo: string,
    number: number,
    sid: string,
    input: DraftResponseInput,
  ) =>
    request<ReviewResponse>(
      'POST',
      `${repoBase(repo)}/reviews/${number}/sessions/${enc(sid)}/responses`,
      input,
    ),
  updateResponse: (
    repo: string,
    number: number,
    sid: string,
    respId: string,
    input: DraftResponseInput,
  ) =>
    request<ReviewResponse>(
      'PUT',
      `${repoBase(repo)}/reviews/${number}/sessions/${enc(sid)}/responses/${enc(respId)}`,
      input,
    ),
  deleteResponse: (repo: string, number: number, sid: string, respId: string) =>
    request<void>(
      'DELETE',
      `${repoBase(repo)}/reviews/${number}/sessions/${enc(sid)}/responses/${enc(respId)}`,
    ),
};
