//! Generation, hashing, and parsing of per-author API tokens.
//!
//! Tokens are long-lived bearer credentials bound to an [`Author`].
//! They substitute for whatever per-request identity `--auth-mode`
//! would otherwise determine — useful for MCP agents and CI
//! integrations that can't go through the interactive OIDC flow.
//!
//! Format on the wire: `kata_pat_<64 hex chars>` (32 bytes of
//! entropy, hex-encoded). Hex is the trade-off vs. base64-url: a few
//! extra characters of length in exchange for a familiar alphabet
//! and zero ambiguity in URL contexts.
//!
//! Storage: the plaintext is shown to the user exactly once at
//! creation and is never persisted. SHA-256 of the plaintext (hex,
//! 64 chars) lands in `api_tokens.token_hash`. Lookups happen by
//! hashing the presented Bearer / `?token=` value and matching the
//! column.

use chrono::Utc;
use kata_core::{ApiToken, ApiTokenId, Author};
use sha2::{Digest, Sha256};
use uuid::{Uuid, timestamp::Timestamp};

/// Wire prefix every Kata-issued token starts with. Lets the auth
/// path early-reject obviously-not-a-Kata-token Bearer values
/// without hitting storage, and makes leaked secrets obvious in
/// logs.
pub const TOKEN_PREFIX: &str = "kata_pat_";

/// Length of the random portion of the plaintext, in hex chars
/// (32 bytes → 64 hex chars).
const HEX_LEN: usize = 64;

/// Number of leading plaintext chars stored alongside the hash so
/// `kata token list` can show "the first bit of this token". Long
/// enough for a human to recognise their tokens; short enough that
/// brute-forcing the remainder still demands the rest of the entropy.
const PREFIX_VISIBLE_LEN: usize = TOKEN_PREFIX.len() + 4;

/// A freshly-generated token, paired with the row that's about to
/// be persisted. The `plaintext` is the only chance the caller has
/// to show it to the user — once `token` is stored, the plaintext
/// is gone for good.
pub struct NewToken {
    pub plaintext: String,
    pub token: ApiToken,
}

/// Mint a new token for `author` with the human-readable `name`
/// label. Returns the plaintext (caller's responsibility to surface
/// it once and discard) and the `ApiToken` row to persist via
/// [`kata_service::ReviewService::store_api_token`].
pub fn mint(author: Author, name: String) -> NewToken {
    // 32 random bytes from two UUID v4s. UUIDs use a cryptographic
    // RNG under the hood (`getrandom`), so we get the same entropy
    // we'd get from pulling `OsRng` directly — without taking on
    // `rand` as a dep just for this.
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    let plaintext = format!("{TOKEN_PREFIX}{}", hex::encode(bytes));
    let token_hash = hash(&plaintext);
    let prefix = plaintext[..PREFIX_VISIBLE_LEN].to_string();
    let token = ApiToken {
        token_id: ApiTokenId::new(Uuid::new_v7(Timestamp::now(uuid::NoContext)).to_string()),
        author,
        name,
        token_hash,
        prefix,
        created_at: Utc::now(),
        last_used_at: None,
        revoked_at: None,
    };
    NewToken { plaintext, token }
}

/// Hash a plaintext token to its storage form. Public for tests
/// and for the auth extractor which needs to hash an inbound
/// Bearer header before querying.
pub fn hash(plaintext: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(plaintext.as_bytes());
    hex::encode(hasher.finalize())
}

/// Return `Some(stripped)` when `value` looks like a Kata-issued
/// token (i.e. starts with [`TOKEN_PREFIX`] and is the right
/// length). `None` for anything else — used to early-reject
/// foreign Bearer values without hitting storage.
pub fn looks_like_token(value: &str) -> bool {
    value.starts_with(TOKEN_PREFIX) && value.len() == TOKEN_PREFIX.len() + HEX_LEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_produces_unique_tokens() {
        let a = mint(Author::new("alice@example.com"), "ci".into());
        let b = mint(Author::new("alice@example.com"), "ci".into());
        assert_ne!(a.plaintext, b.plaintext);
        assert_ne!(a.token.token_id.as_str(), b.token.token_id.as_str());
        assert_ne!(a.token.token_hash, b.token.token_hash);
    }

    #[test]
    fn plaintext_starts_with_prefix() {
        let t = mint(Author::new("alice@example.com"), "ci".into());
        assert!(t.plaintext.starts_with(TOKEN_PREFIX));
        assert_eq!(t.plaintext.len(), TOKEN_PREFIX.len() + HEX_LEN);
    }

    #[test]
    fn stored_prefix_matches_plaintext_prefix() {
        let t = mint(Author::new("alice@example.com"), "ci".into());
        assert!(t.plaintext.starts_with(&t.token.prefix));
        assert_eq!(t.token.prefix.len(), PREFIX_VISIBLE_LEN);
    }

    #[test]
    fn hash_is_deterministic_and_matches_stored() {
        let t = mint(Author::new("alice@example.com"), "ci".into());
        assert_eq!(hash(&t.plaintext), t.token.token_hash);
        // Hex-encoded SHA-256 is 64 chars regardless of input.
        assert_eq!(t.token.token_hash.len(), 64);
    }

    #[test]
    fn looks_like_token_accepts_minted_plaintext() {
        let t = mint(Author::new("alice@example.com"), "ci".into());
        assert!(looks_like_token(&t.plaintext));
    }

    #[test]
    fn looks_like_token_rejects_foreign_bearer_values() {
        assert!(!looks_like_token(""));
        assert!(!looks_like_token("not-a-token"));
        assert!(!looks_like_token("kata_pat_short"));
        // Right prefix, wrong length:
        assert!(!looks_like_token(&format!("{TOKEN_PREFIX}{}", "a".repeat(63))));
        assert!(!looks_like_token(&format!("{TOKEN_PREFIX}{}", "a".repeat(65))));
    }
}
