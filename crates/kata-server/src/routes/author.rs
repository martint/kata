use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use kata_core::Author;

use crate::auth::AuthMode;
use crate::cookies::{SESSION_COOKIE, verify as verify_cookie};
use crate::error::AppError;
use crate::state::AppState;
use crate::tokens;

/// Extractor yielding just the acting author — for handlers that only
/// attribute writes and don't gate on review-creator privileges. The
/// per-mode resolution order lives in [`resolve_author`].
pub struct ViewerAuthor(pub Author);

impl FromRequestParts<AppState> for ViewerAuthor {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(ViewerAuthor(resolve_author(parts, state).await?))
    }
}

/// The acting identity plus whether it carries global-admin
/// privileges (creator-equivalent on every review). Used by handlers
/// that gate on review-creator. `is_admin` is decided here — the one
/// point where identity is resolved — so it's uniform across every
/// auth mode, folding together the per-mode admin sources: the static
/// email allowlist (all modes) and, in `trust-forwarded-header` mode,
/// a proxy-supplied group header.
pub struct Actor {
    pub author: Author,
    pub is_admin: bool,
}

impl FromRequestParts<AppState> for Actor {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let author = resolve_author(parts, state).await?;
        let is_admin = compute_is_admin(parts, state, &author);
        Ok(Actor { author, is_admin })
    }
}

/// Resolve the request's author identity. Lookups happen in this
/// order:
///
/// 1. `Authorization: Bearer <token>` — if present and the token
///    matches an unrevoked row in `api_tokens`, the bound author
///    wins regardless of `--auth-mode`. Bearer values that don't
///    look like Kata-issued tokens fall through to (3); values that
///    look like one but don't authenticate produce 401.
/// 2. `?token=<token>` — same semantics as (1) but on the query
///    string, for MCP clients that can only set query params.
/// 3. The configured [`AuthMode`]:
///    - **`TrustClient`**: an `X-Review-Author` header wins;
///      otherwise the server's configured default is used.
///    - **`TrustForwardedHeader`**: the configured trusted header
///      (default `X-Forwarded-Email`) is the only source. Missing
///      header is a 401.
async fn resolve_author(parts: &Parts, state: &AppState) -> Result<Author, AppError> {
    if let Some(author) = try_authenticate_token(parts, state).await? {
        return Ok(author);
    }
    match state.auth.mode {
        AuthMode::TrustClient => {
            if let Some(value) = parts.headers.get("x-review-author") {
                let s = value.to_str().map_err(|_| {
                    AppError::from(kata_service::ServiceError::BadRequest(
                        "x-review-author header is not valid utf-8".into(),
                    ))
                })?;
                return Ok(Author::new(s.to_owned()));
            }
            Ok(state.default_author.clone())
        }
        AuthMode::TrustForwardedHeader => {
            let header = &state.auth.trusted_header;
            let value = parts.headers.get(header.as_str()).ok_or_else(|| {
                AppError::from(kata_service::ServiceError::Unauthorized(format!(
                    "missing {header} header (auth-mode=trust-forwarded-header)",
                )))
            })?;
            let s = value.to_str().map_err(|_| {
                AppError::from(kata_service::ServiceError::BadRequest(format!(
                    "{header} header is not valid utf-8",
                )))
            })?;
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err(AppError::from(kata_service::ServiceError::Unauthorized(
                    format!("{header} header is empty"),
                )));
            }
            Ok(Author::new(trimmed.to_owned()))
        }
        AuthMode::Oidc => {
            let rt = state.oidc.as_ref().ok_or_else(|| {
                AppError::from(kata_service::ServiceError::Internal(
                    "auth-mode=oidc but no OIDC runtime is configured".into(),
                ))
            })?;
            let cookie_value = extract_cookie(parts, SESSION_COOKIE).ok_or_else(|| {
                AppError::from(kata_service::ServiceError::Unauthorized(
                    "no session cookie — log in at /auth/login".into(),
                ))
            })?;
            let payload = verify_cookie(
                &rt.config.session_secret,
                &cookie_value,
                chrono::Utc::now(),
            )
            .map_err(|e| {
                AppError::from(kata_service::ServiceError::Unauthorized(format!(
                    "invalid session cookie: {e} — log in at /auth/login",
                )))
            })?;
            Ok(Author::new(payload.author))
        }
    }
}

/// Whether the resolved `author` acts with global-admin privileges.
/// Admin-by-email ([`AuthConfig::is_admin_email`]) applies in every
/// mode. Admin-by-group reads the proxy groups header and only applies
/// in `trust-forwarded-header` mode, where the upstream-IP allowlist
/// middleware has already vouched for the proxy that set it — so the
/// groups header is exactly as trustworthy as the email header beside
/// it. A non-UTF-8 groups header is treated as "no groups".
fn compute_is_admin(parts: &Parts, state: &AppState, author: &Author) -> bool {
    if state.auth.is_admin_email(author) {
        return true;
    }
    if state.auth.mode == AuthMode::TrustForwardedHeader
        && let Some(value) = parts.headers.get(state.auth.groups_header.as_str())
        && let Ok(s) = value.to_str()
    {
        return state.auth.is_admin_group(s);
    }
    false
}

fn extract_cookie(parts: &Parts, name: &str) -> Option<String> {
    let header = parts.headers.get(axum::http::header::COOKIE)?;
    let s = header.to_str().ok()?;
    for pair in s.split(';') {
        let pair = pair.trim();
        let (k, v) = pair.split_once('=')?;
        if k == name {
            return Some(v.to_owned());
        }
    }
    None
}

/// Inspect the request for a Kata-issued API token in either
/// `Authorization: Bearer` or `?token=`. Returns:
///
/// - `Ok(Some(author))` — a token was presented and authenticated.
/// - `Ok(None)` — no token-like value was presented; fall through
///   to the configured auth mode.
/// - `Err(401)` — a token-like value was presented but didn't
///   authenticate (unknown, malformed, or revoked). We fail closed
///   in this case rather than silently dropping to the auth-mode
///   fallback, because a client *intending* to authenticate via
///   token should know their token isn't being honoured.
async fn try_authenticate_token(
    parts: &Parts,
    state: &AppState,
) -> Result<Option<Author>, AppError> {
    let presented = extract_bearer(parts).or_else(|| extract_query_token(parts));
    let Some(plaintext) = presented else {
        return Ok(None);
    };
    if !tokens::looks_like_token(&plaintext) {
        // A Bearer value that doesn't look like a Kata token may be
        // intended for some upstream proxy or middleware — pass it
        // through so the auth-mode path can decide.
        return Ok(None);
    }
    let hash = tokens::hash(&plaintext);
    let token = state
        .service
        .authenticate_api_token(&hash)
        .await
        .map_err(AppError::from)?;
    match token {
        Some(t) => Ok(Some(t.author)),
        None => Err(AppError::from(kata_service::ServiceError::Unauthorized(
            "api token unknown or revoked".into(),
        ))),
    }
}

fn extract_bearer(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(axum::http::header::AUTHORIZATION)?;
    let s = value.to_str().ok()?;
    let token = s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer "))?;
    Some(token.trim().to_owned())
}

fn extract_query_token(parts: &Parts) -> Option<String> {
    let query = parts.uri.query()?;
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let key = it.next()?;
        if key != "token" {
            continue;
        }
        let raw = it.next()?;
        // URL-decode the simplest cases. `?token=` values can only
        // contain hex + the `kata_pat_` prefix, which are entirely
        // unreserved, so percent-decoding is rarely needed; but
        // accept `%2B`-style sequences for forward compatibility
        // with operators who URL-encode out of habit.
        return Some(percent_decode(raw));
    }
    None
}

fn percent_decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) =
                (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
            {
                out.push((h * 16 + l) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
