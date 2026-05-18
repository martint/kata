-- Author-attached context notes ("annotations"). Unlike comments these
-- skip the session/draft cycle entirely: the review creator writes one,
-- it goes live on submit, and only the creator can edit or delete it.
-- No flag (severity makes no sense for context notes) and no responses
-- (annotations are one-way; if a reviewer wants to discuss, they file a
-- regular comment).
CREATE TABLE annotations (
    annotation_id TEXT NOT NULL PRIMARY KEY,
    repo_id TEXT NOT NULL,
    review_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    author TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    patchset INTEGER NOT NULL,
    anchor_change_id TEXT NOT NULL,
    anchor_commit_id TEXT NOT NULL,
    -- Optional anchor target, mirroring comments' shape: `file` is NULL
    -- for review-wide annotations; `side` + `line_start`/`line_end` are
    -- NULL for whole-file annotations.
    file TEXT,
    side TEXT,
    line_start INTEGER,
    line_end INTEGER,
    body TEXT NOT NULL
);

CREATE INDEX annotations_by_review ON annotations (repo_id, review_id);
