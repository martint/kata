-- Tracks each reviewer's last-seen jj op-id per review, so open_review can
-- report what kinds of operations happened in the repo since the reviewer
-- was last here. One row per (review, author); rewritten on every open.
CREATE TABLE review_visits (
    review_id     TEXT NOT NULL,
    author        TEXT NOT NULL,
    op_id         TEXT NOT NULL,
    visited_at    TEXT NOT NULL,
    PRIMARY KEY (review_id, author),
    FOREIGN KEY (review_id) REFERENCES reviews(review_id) ON DELETE CASCADE
);
