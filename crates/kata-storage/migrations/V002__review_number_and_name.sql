-- V002: decouple review identity from the bookmark slug.
--
-- Reviews now carry two new pieces of metadata:
--
--   * `number` — per-repo monotonic counter. Stable across renames,
--     and the value that URLs / breadcrumbs use (so two reviews on the
--     same bookmark can coexist as #1 and #2). The internal `review_id`
--     stays opaque (UUID v7 for new reviews, the bookmark slug for any
--     review carried in from a pre-numbering archive).
--   * `name` — human-readable label, defaults to the bookmark slug.
--     Editable; never affects identity.
--
-- For existing rows we backfill `name = review_id` (the slug was the
-- best available label) and assign numbers in creation order via a
-- window function. A unique partial index on `(repo_id, number)`
-- enforces per-repo uniqueness once backfill is done.

ALTER TABLE reviews ADD COLUMN name TEXT NOT NULL DEFAULT '';
ALTER TABLE reviews ADD COLUMN number INTEGER NOT NULL DEFAULT 0;

UPDATE reviews SET name = review_id WHERE name = '';

UPDATE reviews
SET number = sub.rn
FROM (
    SELECT repo_id,
           review_id,
           row_number() OVER (PARTITION BY repo_id ORDER BY created_at) AS rn
    FROM reviews
) AS sub
WHERE reviews.repo_id = sub.repo_id AND reviews.review_id = sub.review_id;

CREATE UNIQUE INDEX reviews_number_per_repo ON reviews (repo_id, number);
