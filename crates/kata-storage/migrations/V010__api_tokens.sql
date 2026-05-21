-- Per-author API tokens. Long-lived bearer credentials bound to an
-- author identity, primarily for MCP agents and CI integrations that
-- can't go through the interactive OIDC flow. Tokens are server-side
-- credentials, not user content — they intentionally don't appear in
-- the archive format (a token issued on host A is meaningless on host
-- B even after a restore).
--
-- Storage shape:
--
-- * `token_id` is the public identifier — UUID v7, printed by
--   `kata token list`, used as the argument to `kata token revoke`.
--   Safe to log.
-- * `token_hash` is SHA-256 of the plaintext (hex). Lookups happen by
--   hash; the plaintext is shown to the user exactly once at creation
--   and never persisted.
-- * `prefix` is the first ~12 chars of the plaintext (`kata_pat_AaBb`),
--   enough for humans to recognise their tokens in the listing
--   without enabling a guessing attack.
-- * `last_used_at` ticks every time the token authenticates a
--   request; useful for spotting stale tokens.
-- * `revoked_at` is a soft delete — keep the row so audit logs can
--   still resolve `token_id` references after revocation.
CREATE TABLE api_tokens (
    token_id TEXT NOT NULL PRIMARY KEY,
    author TEXT NOT NULL,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    prefix TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at TEXT
);

-- Lookups happen on every authenticated request, so the hash needs
-- to be indexed. UNIQUE on `token_hash` above gives us this implicitly,
-- but listing by author (e.g. `kata token list --author bob`) wants
-- its own index.
CREATE INDEX api_tokens_by_author ON api_tokens (author);
