-- V001: initial schema for Kata's per-repo / per-review / per-comment store.
--
-- One physical SQLite database is shared across all repos: rows are keyed
-- by `repo_id`, which is the hash of a `.jj/repo` canonical path (see
-- `compute_repo_id` in lib.rs). Reviews, sessions, comments, and responses
-- all inherit that scope.
--
-- `text` / `integer` / `real` columns store the same primitive shapes the
-- domain types serialize to today. Composite documents that the runtime
-- treats as opaque blobs (the patchset history, the manifest summary) live
-- in `*_json` columns — keeping them out of the normalized schema avoids
-- having to break the migration every time a new field shows up in
-- `kata_core::documents`. Anything we query against (status, author,
-- in_reply_to) is a real column with an index.

PRAGMA foreign_keys = ON;

CREATE TABLE repos (
    repo_id TEXT NOT NULL PRIMARY KEY,
    canonical_path TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE reviews (
    repo_id TEXT NOT NULL,
    review_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    revset TEXT NOT NULL,
    bookmark TEXT,
    -- Author-written markdown. Optional; older manifests didn't have one.
    summary TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    current_patchset INTEGER NOT NULL,
    -- Patchset[] serialized as JSON. The runtime reads it as one unit and
    -- writes it back atomically on `update_review`; no value in
    -- normalizing into a child table until we have queries that touch a
    -- single patchset by id (we don't today).
    patchsets_json TEXT NOT NULL,
    PRIMARY KEY (repo_id, review_id),
    FOREIGN KEY (repo_id) REFERENCES repos(repo_id) ON DELETE CASCADE
);

CREATE TABLE sessions (
    session_id TEXT NOT NULL PRIMARY KEY,
    repo_id TEXT NOT NULL,
    review_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    author TEXT NOT NULL,
    -- One of 'draft' | 'published' | 'discarded'. Matches the kebab-case
    -- serde form of `SessionStatus`.
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    published_at TEXT,
    FOREIGN KEY (repo_id, review_id) REFERENCES reviews(repo_id, review_id) ON DELETE CASCADE
);

-- Atomicity for open_or_create_session: at most one *draft* session per
-- (repo, review, author) at a time. Published / discarded sessions don't
-- participate, so the index is partial.
CREATE UNIQUE INDEX sessions_one_draft_per_author
    ON sessions (repo_id, review_id, author)
    WHERE status = 'draft';

CREATE INDEX sessions_by_review ON sessions (repo_id, review_id);

CREATE TABLE comments (
    comment_id TEXT NOT NULL PRIMARY KEY,
    repo_id TEXT NOT NULL,
    review_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    author TEXT NOT NULL,
    created_at TEXT NOT NULL,
    patchset INTEGER NOT NULL,
    anchor_change_id TEXT NOT NULL,
    anchor_commit_id TEXT NOT NULL,
    -- Optional anchor target. `file` is NULL for review-wide comments;
    -- `side` + `line_start`/`line_end` are NULL for file-wide comments.
    file TEXT,
    side TEXT,
    line_start INTEGER,
    line_end INTEGER,
    -- One of 'must-do' | 'suggestion' | 'other'.
    flag TEXT NOT NULL,
    body TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE INDEX comments_by_session ON comments (session_id);
CREATE INDEX comments_by_review ON comments (repo_id, review_id);

CREATE TABLE responses (
    response_id TEXT NOT NULL PRIMARY KEY,
    repo_id TEXT NOT NULL,
    review_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    in_reply_to TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    author TEXT NOT NULL,
    created_at TEXT NOT NULL,
    -- One of 'comment' | 'resolve' | 'unresolve' | 'wont-fix'.
    action TEXT NOT NULL,
    body TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
    FOREIGN KEY (in_reply_to) REFERENCES comments(comment_id) ON DELETE CASCADE
);

CREATE INDEX responses_by_session ON responses (session_id);
CREATE INDEX responses_by_comment ON responses (in_reply_to);
