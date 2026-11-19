const BASE = "http://127.0.0.1:7878";
const enc = encodeURIComponent;
const repoBase = (repo) => `/api/repos/${enc(repo)}`;

export class KataError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

// No identity header: in trust-client mode the server acts as its --author, as for the browser.
async function request(method, path, body) {
  const options = { method };
  if (body !== undefined) {
    options.headers = { "content-type": "application/json" };
    options.body = JSON.stringify(body);
  }
  let response;
  try {
    response = await fetch(`${BASE}${path}`, options);
  } catch (e) {
    throw new KataError(0, `Can't reach kata at ${BASE}`);
  }
  if (!response.ok) {
    let detail = `HTTP ${response.status}`;
    try {
      const json = await response.json();
      detail = json.error ?? detail;
    } catch {}
    throw new KataError(response.status, detail);
  }
  if (response.status === 204) return null;
  return response.json();
}

const query = (params) => {
  const parts = Object.entries(params).filter(([, v]) => v != null).map(([k, v]) => `${k}=${enc(v)}`);
  return parts.length ? `?${parts.join("&")}` : "";
};
const reviewBase = (repo, number) => `${repoBase(repo)}/reviews/${number}`;
const sessionBase = (repo, number, session) => `${reviewBase(repo, number)}/sessions/${enc(session)}`;

async function requestText(path) {
  let response;
  try {
    response = await fetch(`${BASE}${path}`);
  } catch (e) {
    throw new KataError(0, `Can't reach kata at ${BASE}`);
  }
  if (!response.ok) throw new KataError(response.status, `HTTP ${response.status}`);
  return response.text();
}

export const kata = {
  fileText: (repo, commit, path) => requestText(`${repoBase(repo)}/files${query({ commit, path })}`),
  repos: () => request("GET", "/api/repos"),
  reviews: (repo) => request("GET", `${repoBase(repo)}/reviews`),
  review: (repo, number, version = {}) => request("GET", `${reviewBase(repo, number)}${query({ patchset: version.patchset, compare: version.compare })}`),
  refreshReview: (repo, number) => request("POST", `${reviewBase(repo, number)}/refresh`, {}),
  archiveReview: (repo, number) => request("POST", `${reviewBase(repo, number)}/archive`),
  unarchiveReview: (repo, number) => request("DELETE", `${reviewBase(repo, number)}/archive`),
  deleteReview: (repo, number) => request("DELETE", reviewBase(repo, number)),
  commitDiff: (repo, number, change) => request("GET", `${reviewBase(repo, number)}/commits/${enc(change)}/diff`),
  fileDiff: (repo, number, path, version = {}) =>
    request("GET", `${reviewBase(repo, number)}/file-diff${query({ path, ps: version.patchset, compare: version.compare })}`),

  startSession: (repo, number) => request("POST", `${reviewBase(repo, number)}/sessions`),
  publishSession: (repo, number, session) => request("POST", `${sessionBase(repo, number, session)}/publish`),
  discardSession: (repo, number, session) => request("POST", `${sessionBase(repo, number, session)}/discard`),

  createComment: (repo, number, session, input) => request("POST", `${sessionBase(repo, number, session)}/comments`, input),
  updateComment: (repo, number, session, comment, input) =>
    request("PUT", `${sessionBase(repo, number, session)}/comments/${enc(comment)}`, input),
  deleteComment: (repo, number, session, comment) => request("DELETE", `${sessionBase(repo, number, session)}/comments/${enc(comment)}`),
  createResponse: (repo, number, session, input) => request("POST", `${sessionBase(repo, number, session)}/responses`, input),
  deleteResponse: (repo, number, session, response) =>
    request("DELETE", `${sessionBase(repo, number, session)}/responses/${enc(response)}`),
};

// The next event from the server's stream (SSE "data:" line), or null when none came within the window.
// curl is killed as soon as one arrives, so nothing lingers between calls.
const WAIT_SECONDS = 280;
export async function nextEvent(exec) {
  const script = `exec 3< <(exec curl -sN --max-time ${WAIT_SECONDS} ${BASE}/api/events); pid=$!
while IFS= read -r line <&3; do case "$line" in data:*) echo "\${line#data: }"; kill $pid 2>/dev/null; exit 0;; esac; done`;
  const result = await exec("bash", ["-c", script], { timeout: (WAIT_SECONDS + 10) * 1000 });
  const line = result.stdout.trim();
  if (!line) {
    if (result.code !== 0) throw new KataError(0, `Can't reach kata at ${BASE}`);
    return null;
  }
  try {
    return JSON.parse(line);
  } catch {
    return null;
  }
}

export function repoRoot(repo) {
  return repo.canonical_path.replace(/\/\.jj\/repo$/, "");
}

export function authorName(author) {
  return (author ?? "").replace(/\s*<[^>]*>\s*$/, "") || author || "";
}
