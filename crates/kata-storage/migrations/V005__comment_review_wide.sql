-- Distinguishes "review-wide" comments (about the whole review, no
-- specific commit) from commit-level comments (file/lines/side all NULL
-- but pinned to a specific change at anchor_change_id). The data shape
-- was previously indistinguishable; existing comments default to 0
-- (commit-level), which is the historical interpretation.
ALTER TABLE comments ADD COLUMN review_wide INTEGER NOT NULL DEFAULT 0;
