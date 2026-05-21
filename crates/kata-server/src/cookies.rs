//! Signed-cookie machinery used by the OIDC session.
//!
//! Cookies on the wire have the shape `<base64url(payload)>.<base64url(hmac)>`
//! where `payload` is JSON `{ "a": "<email>", "exp": <unix-seconds> }` and
//! `hmac` is HMAC-SHA256(secret, payload) — same shape as oauth2-proxy's
//! cookie or any other "stateless session" you've squinted at in the
//! wild. The payload is signed but **not encrypted**: the email is the
//! user's own, and shielding it from the user reading their own cookies
//! buys nothing here.
//!
//! Secret rotation: changing the secret invalidates every outstanding
//! cookie, which is the desired behaviour on intentional rotation (forces
//! re-login). The CLI takes a single secret; ops that want zero-downtime
//! rotation can run a brief overlap period by deploying a fleet with the
//! new secret and accepting that users with old cookies will get one
//! redirect through the OIDC flow.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, TimeZone, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Cookie name. Hard-coded so the SPA's logout-flow can name it
/// without a config round-trip.
pub const SESSION_COOKIE: &str = "kata_session";

/// Payload carried inside the signed cookie. Kept narrow because the
/// cookie is on every request — only the bits the auth path needs
/// per-request live here. Everything else (display name, group
/// membership, raw ID-token claims) is intentionally not persisted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionPayload {
    /// Author identity. Stamped onto every write the session author
    /// makes. Comes from the OIDC `email` claim at login time.
    #[serde(rename = "a")]
    pub author: String,
    /// Unix-seconds expiry. The session is rejected at or after this
    /// instant regardless of the cookie's own `Max-Age`; the cookie's
    /// `Max-Age` is set to match so the browser also stops sending it.
    #[serde(rename = "exp")]
    pub expires_at: i64,
}

impl SessionPayload {
    pub fn expires_at_utc(&self) -> Option<DateTime<Utc>> {
        Utc.timestamp_opt(self.expires_at, 0).single()
    }

    /// Whether the payload's `expires_at` has already elapsed.
    /// Returns `true` when the timestamp is unparseable too — a
    /// degenerate payload should not authenticate.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at_utc() {
            Some(t) => t <= now,
            None => true,
        }
    }
}

/// Errors that can come out of [`verify_cookie`]. Each variant is a
/// distinct reason to reject the presented cookie; the auth path
/// uses the variant to decide between "401, try logging in again"
/// (for expired) and "tamper / mismatch — likely an attacker"
/// (for the others).
#[derive(Debug, thiserror::Error)]
pub enum CookieError {
    #[error("cookie missing payload/signature separator")]
    Malformed,
    #[error("cookie base64-decode failed: {0}")]
    Base64(String),
    #[error("cookie signature did not verify")]
    BadSignature,
    #[error("cookie payload JSON did not parse: {0}")]
    BadPayload(String),
    #[error("cookie has expired")]
    Expired,
}

/// Sign a payload into wire form. The `secret` is opaque bytes;
/// callers typically derive it from a CLI/env-supplied string.
pub fn sign(secret: &[u8], payload: &SessionPayload) -> String {
    let body = serde_json::to_vec(payload).expect("SessionPayload serializes");
    let mac = hmac_sha256(secret, &body);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&body),
        URL_SAFE_NO_PAD.encode(mac),
    )
}

/// Reverse of [`sign`]. Verifies the signature, parses the payload,
/// and checks expiry. Returns `Err(Expired)` only when the signature
/// AND the JSON parse both succeed — a tampered cookie that happens
/// to also be expired prefers the tamper diagnostic.
pub fn verify(
    secret: &[u8],
    cookie: &str,
    now: DateTime<Utc>,
) -> Result<SessionPayload, CookieError> {
    let (b64_payload, b64_sig) = cookie.split_once('.').ok_or(CookieError::Malformed)?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(b64_payload)
        .map_err(|e| CookieError::Base64(e.to_string()))?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(b64_sig)
        .map_err(|e| CookieError::Base64(e.to_string()))?;
    let expected = hmac_sha256(secret, &payload_bytes);
    if !constant_time_eq(&expected, &sig_bytes) {
        return Err(CookieError::BadSignature);
    }
    let payload: SessionPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|e| CookieError::BadPayload(e.to_string()))?;
    if payload.is_expired(now) {
        return Err(CookieError::Expired);
    }
    Ok(payload)
}

fn hmac_sha256(secret: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC keyed");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Compare two byte slices in constant time. Defends against the
/// (theoretical, given HMAC-SHA256's fast-fail) timing leak that
/// would let an attacker discover the secret one byte at a time.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).unwrap()
    }

    fn payload(exp: i64) -> SessionPayload {
        SessionPayload {
            author: "alice@example.com".into(),
            expires_at: exp,
        }
    }

    #[test]
    fn sign_verify_round_trip() {
        let secret = b"shhh";
        let p = payload(now().timestamp() + 3600);
        let cookie = sign(secret, &p);
        let back = verify(secret, &cookie, now()).expect("must verify");
        assert_eq!(back, p);
    }

    #[test]
    fn expired_cookie_rejected() {
        let secret = b"shhh";
        let p = payload(now().timestamp() - 1);
        let cookie = sign(secret, &p);
        let err = verify(secret, &cookie, now()).expect_err("expired must reject");
        assert!(matches!(err, CookieError::Expired));
    }

    #[test]
    fn tampered_payload_rejected_with_bad_signature() {
        let secret = b"shhh";
        let p = payload(now().timestamp() + 3600);
        let cookie = sign(secret, &p);
        // Flip the last byte of the payload before signing.
        let (b64_payload, b64_sig) = cookie.split_once('.').unwrap();
        let mut decoded = URL_SAFE_NO_PAD.decode(b64_payload).unwrap();
        *decoded.last_mut().unwrap() ^= 1;
        let tampered = format!("{}.{}", URL_SAFE_NO_PAD.encode(&decoded), b64_sig);
        let err = verify(secret, &tampered, now()).expect_err("tampered must reject");
        assert!(matches!(err, CookieError::BadSignature));
    }

    #[test]
    fn wrong_secret_rejected() {
        let p = payload(now().timestamp() + 3600);
        let cookie = sign(b"alpha", &p);
        let err = verify(b"beta", &cookie, now()).expect_err("wrong secret must reject");
        assert!(matches!(err, CookieError::BadSignature));
    }

    #[test]
    fn malformed_cookie_rejected() {
        let err = verify(b"shhh", "not-a-cookie", now()).expect_err("must reject");
        assert!(matches!(err, CookieError::Malformed));
    }

    #[test]
    fn signature_section_garbled_rejects() {
        let p = payload(now().timestamp() + 3600);
        let cookie = sign(b"shhh", &p);
        let (b64_payload, _) = cookie.split_once('.').unwrap();
        let bogus = format!("{}.AAAA", b64_payload);
        let err = verify(b"shhh", &bogus, now()).expect_err("must reject");
        assert!(matches!(err, CookieError::BadSignature));
    }
}
