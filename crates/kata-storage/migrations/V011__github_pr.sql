-- GitHub PR linkage and comment-identity mapping.
--
-- Three additions, all additive:
--
-- 1. `reviews.github_pr` — JSON blob (NULL for native kata reviews)
--    storing the GitHub provenance of a review created via the
--    /api/github/import endpoint. Shape:
--
--      {"owner":"...", "repo":"...", "number":123,
--       "html_url":"https://github.com/.../pull/123",
--       "original_head_sha":"...", "original_base_sha":"..."}
--
--    The two SHAs are captured at import time; phase 6 uses them
--    to detect that the PR head moved before publishing back.
--
-- 2. `comments.external_author` — JSON blob (NULL for native
--    kata-authored comments) carrying ghost-author rendering data
--    for comments imported from a non-kata source. Phase 5 will
--    populate this for github-imported comments; native draft /
--    publish paths leave it NULL. Shape:
--
--      {"source":"github",
--       "login":"...", "id":123,
--       "avatar_url":"...", "html_url":"..."}
--
-- 3. `github_comment_map` — identity mapping between kata
--    comments/responses and the GitHub objects they correspond to.
--    Populated on import (so refresh doesn't double-insert) and on
--    publish (so replies + resolve toggles target the right
--    upstream id). The `kind` discriminator lets us tell a review
--    thread's anchor comment apart from a reply, and from an
--    issue-style top-level comment that has no review wrapper.
--    Indexed on (repo_id, github_node_id) for the dedup lookup
--    during refresh; a secondary index on kata_comment_id supports
--    the "find the upstream id for this draft response" lookup
--    during publish.
ALTER TABLE reviews ADD COLUMN github_pr TEXT;
ALTER TABLE comments ADD COLUMN external_author TEXT;

CREATE TABLE github_comment_map (
    repo_id TEXT NOT NULL,
    github_node_id TEXT NOT NULL,
    github_rest_id INTEGER,
    kind TEXT NOT NULL,
    kata_comment_id TEXT,
    kata_response_id TEXT,
    review_id TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    thread_node_id TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (repo_id, github_node_id),
    FOREIGN KEY (repo_id, review_id) REFERENCES reviews (repo_id, review_id) ON DELETE CASCADE
);

CREATE INDEX github_comment_map_by_kata_comment
    ON github_comment_map (repo_id, kata_comment_id)
    WHERE kata_comment_id IS NOT NULL;

CREATE INDEX github_comment_map_by_kata_response
    ON github_comment_map (repo_id, kata_response_id)
    WHERE kata_response_id IS NOT NULL;
