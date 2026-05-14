-- V003: track per-review archival.
--
-- A creator can archive a review they no longer want to actively
-- pursue: the home screen hides it by default, the viewer renders it
-- in a muted state, and writes (new sessions / comments / responses)
-- are rejected until it is unarchived. We store the time of the most
-- recent archive transition rather than a boolean so listings can
-- order archived reviews by recency and so we keep an audit-ish trace
-- of when the change happened.
--
-- Existing rows are active (NULL `archived_at`) — nothing to backfill.

ALTER TABLE reviews ADD COLUMN archived_at TEXT;
