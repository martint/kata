-- Rebuild review_visits so its foreign key actually matches the reviews
-- table's primary key. V004 declared `FOREIGN KEY (review_id) REFERENCES
-- reviews(review_id)`, but reviews is keyed on (repo_id, review_id) — a
-- single-column FK against a composite PK is invalid, and with
-- foreign_keys=ON every INSERT failed with "foreign key mismatch". The
-- write site swallowed the error via `let _ = ...`, so the feature has
-- never actually persisted a row in production.
--
-- The old table only ever held data on installs where foreign_keys was
-- OFF (none of ours), so there's nothing to migrate — just drop and
-- recreate with `repo_id` as part of the row and a proper composite FK.
DROP TABLE IF EXISTS review_visits;

CREATE TABLE review_visits (
    repo_id       TEXT NOT NULL,
    review_id     TEXT NOT NULL,
    author        TEXT NOT NULL,
    op_id         TEXT NOT NULL,
    visited_at    TEXT NOT NULL,
    PRIMARY KEY (repo_id, review_id, author),
    FOREIGN KEY (repo_id, review_id) REFERENCES reviews(repo_id, review_id) ON DELETE CASCADE
);
