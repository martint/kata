import type {
  Bookmark,
  Comment,
  CommitDiffView,
  CreateReviewParams,
  DraftCommentInput,
  DraftResponseInput,
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
    super(`api ${status}: ${detail}`);
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

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = { method };
  if (body !== undefined) {
    init.headers = { 'content-type': 'application/json' };
    init.body = JSON.stringify(body);
  }
  const t0 = performance.now();
  const res = await fetch(path, init);
  const tFetch = performance.now() - t0;
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
  const parsed = (await res.json()) as T;
  const tTotal = performance.now() - t0;
  if (tTotal > 100) {
    // eslint-disable-next-line no-console
    console.log(
      `[api] ${method} ${path}: ${tTotal.toFixed(0)}ms (fetch ${tFetch.toFixed(0)}ms, parse ${(tTotal - tFetch).toFixed(0)}ms)`,
    );
  }
  return parsed;
}

const enc = encodeURIComponent;

const repoBase = (repo: string) => `/api/repos/${enc(repo)}`;

export const api = {
  whoami: () => request<WhoAmI>('GET', '/api/whoami'),
  listRepos: () => request<RepoSummary[]>('GET', '/api/repos'),

  listBookmarks: (repo: string) =>
    request<Bookmark[]>('GET', `${repoBase(repo)}/bookmarks`),

  listReviews: (repo: string) =>
    request<ReviewSummary[]>('GET', `${repoBase(repo)}/reviews`),
  createReview: (repo: string, params: CreateReviewParams) =>
    request<ReviewManifest>('POST', `${repoBase(repo)}/reviews`, params),
  openReview: (repo: string, id: string, patchset?: number) => {
    const qs = patchset !== undefined ? `?patchset=${patchset}` : '';
    return request<ReviewView>('GET', `${repoBase(repo)}/reviews/${enc(id)}${qs}`);
  },
  refreshReview: (repo: string, id: string, summary?: string) =>
    request<ReviewManifest>(
      'POST',
      `${repoBase(repo)}/reviews/${enc(id)}/refresh`,
      summary !== undefined ? { summary } : {},
    ),
  updateSummary: (repo: string, id: string, summary: string | null) =>
    request<ReviewManifest>(
      'PUT',
      `${repoBase(repo)}/reviews/${enc(id)}/summary`,
      { summary },
    ),
  commitDiff: (repo: string, reviewId: string, changeId: string) =>
    request<CommitDiffView>(
      'GET',
      `${repoBase(repo)}/reviews/${enc(reviewId)}/commits/${enc(changeId)}/diff`,
    ),
  readFile: (repo: string, commit: string, path: string) =>
    fetchText(`${repoBase(repo)}/files?commit=${enc(commit)}&path=${enc(path)}`),

  startSession: (repo: string, id: string) =>
    request<Session>('POST', `${repoBase(repo)}/reviews/${enc(id)}/sessions`),
  publishSession: (repo: string, rid: string, sid: string) =>
    request<void>(
      'POST',
      `${repoBase(repo)}/reviews/${enc(rid)}/sessions/${enc(sid)}/publish`,
    ),
  discardSession: (repo: string, rid: string, sid: string) =>
    request<void>(
      'POST',
      `${repoBase(repo)}/reviews/${enc(rid)}/sessions/${enc(sid)}/discard`,
    ),

  createComment: (repo: string, rid: string, sid: string, input: DraftCommentInput) =>
    request<Comment>(
      'POST',
      `${repoBase(repo)}/reviews/${enc(rid)}/sessions/${enc(sid)}/comments`,
      input,
    ),
  updateComment: (
    repo: string,
    rid: string,
    sid: string,
    cid: string,
    input: DraftCommentInput,
  ) =>
    request<Comment>(
      'PUT',
      `${repoBase(repo)}/reviews/${enc(rid)}/sessions/${enc(sid)}/comments/${enc(cid)}`,
      input,
    ),
  deleteComment: (repo: string, rid: string, sid: string, cid: string) =>
    request<void>(
      'DELETE',
      `${repoBase(repo)}/reviews/${enc(rid)}/sessions/${enc(sid)}/comments/${enc(cid)}`,
    ),

  createResponse: (repo: string, rid: string, sid: string, input: DraftResponseInput) =>
    request<ReviewResponse>(
      'POST',
      `${repoBase(repo)}/reviews/${enc(rid)}/sessions/${enc(sid)}/responses`,
      input,
    ),
  updateResponse: (
    repo: string,
    rid: string,
    sid: string,
    respId: string,
    input: DraftResponseInput,
  ) =>
    request<ReviewResponse>(
      'PUT',
      `${repoBase(repo)}/reviews/${enc(rid)}/sessions/${enc(sid)}/responses/${enc(respId)}`,
      input,
    ),
  deleteResponse: (repo: string, rid: string, sid: string, respId: string) =>
    request<void>(
      'DELETE',
      `${repoBase(repo)}/reviews/${enc(rid)}/sessions/${enc(sid)}/responses/${enc(respId)}`,
    ),
};
