//! OIDC client + login/callback/logout routes.
//!
//! Discovers the issuer at startup, then handles the authorization-
//! code flow with PKCE. On callback we validate the ID token and
//! mint a signed session cookie (see [`crate::cookies`]); the
//! cookie's `email` claim becomes the actor on every subsequent
//! request via [`crate::routes::author::ViewerAuthor`].
//!
//! State between `/auth/login` and `/auth/callback` (CSRF token,
//! nonce, PKCE verifier, post-login redirect target) lives in an
//! in-process map keyed by CSRF token, with a short TTL. The
//! upside is "no extra cookie traffic and no per-state DB write";
//! the downside is that an in-flight OIDC dance does not survive a
//! server restart. Acceptable: the user just runs `/auth/login`
//! again. If we ever go multi-instance, this map becomes a stamped
//! Redis row or — more likely — a separate state cookie.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
// Reqwest client + provider metadata are constructed once at startup
// and shared via Arc; the typed `CoreClient` is *rebuilt* per request
// from that cached metadata. Rebuilding is allocation-only (no
// network) and sidesteps a 17-parameter typestate `Client<…>` we'd
// otherwise have to spell out as a struct field type.
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::auth::OidcConfig;
use crate::cookies::{SESSION_COOKIE, SessionPayload, sign};

/// Built-once OIDC provider metadata + the in-flight state map.
/// Lives inside [`crate::state::AppState`] when (and only when) the
/// server runs in `AuthMode::Oidc`. The typed `CoreClient` is
/// rebuilt per request from `metadata`; rebuilding is allocation-
/// only and lets us avoid spelling out the 17-parameter typestate
/// `Client<…>` as a struct field.
#[derive(Clone)]
pub struct OidcRuntime {
    pub config: OidcConfig,
    metadata: Arc<CoreProviderMetadata>,
    http: reqwest::Client,
    /// CSRF token → state record (nonce, PKCE verifier, optional
    /// post-login redirect path). Locked because we mutate during
    /// `/login` and `/callback`. Contention is per-OIDC-dance, not
    /// per-request, so a single mutex is fine.
    pending: Arc<Mutex<HashMap<String, PendingLogin>>>,
}

/// What we need to remember between sending the user to the IdP and
/// receiving their callback. Mirrors the openidconnect crate's
/// expectations on `exchange_code` / `id_token().claims(..., nonce)`.
struct PendingLogin {
    nonce: Nonce,
    pkce_verifier: PkceCodeVerifier,
    /// Where the user wanted to go before we hijacked them through
    /// `/auth/login`. Honoured on a successful callback by 302-ing
    /// there instead of `/`. Validated to a same-origin path on
    /// entry so an attacker can't ride a victim through
    /// `/auth/login?next=https://evil.example`.
    next: Option<String>,
    /// When this entry was created. Entries older than `MAX_AGE`
    /// are swept at lookup time; we don't run a separate timer for
    /// this — the natural login traffic is the timer.
    created_at: Instant,
}

const MAX_AGE: Duration = Duration::from_secs(10 * 60);

/// Build the OIDC runtime: fetch the issuer's discovery document
/// once at startup and stash the resulting metadata. Fatal if
/// discovery fails — the server can't run in OIDC mode without it.
pub async fn build_runtime(config: OidcConfig) -> Result<OidcRuntime, String> {
    let http = reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("building HTTP client for OIDC: {e}"))?;
    let issuer = IssuerUrl::new(config.issuer.clone())
        .map_err(|e| format!("--oidc-issuer is not a valid URL: {e}"))?;
    let metadata = CoreProviderMetadata::discover_async(issuer, &http)
        .await
        .map_err(|e| format!("OIDC discovery against {} failed: {e}", config.issuer))?;
    // Validate the redirect URI now so a typo dies at startup rather
    // than at the first /login attempt.
    RedirectUrl::new(config.redirect_uri.clone())
        .map_err(|e| format!("--oidc-redirect-uri is not a valid URL: {e}"))?;
    Ok(OidcRuntime {
        config,
        metadata: Arc::new(metadata),
        http,
        pending: Arc::new(Mutex::new(HashMap::new())),
    })
}


#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Path the SPA wanted to land on before the redirect. Honoured
    /// on successful callback. Sanitised to "starts-with `/`" so
    /// attackers can't pivot to an arbitrary origin.
    next: Option<String>,
}

/// `/auth/login`: build the IdP authorization URL with a fresh
/// CSRF token + nonce + PKCE pair, stash the latter in the
/// in-flight map, and 302 the user there.
pub async fn login(
    State(rt): State<OidcRuntime>,
    Query(q): Query<LoginQuery>,
) -> Response {
    let redirect = match RedirectUrl::new(rt.config.redirect_uri.clone()) {
        Ok(r) => r,
        Err(e) => return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("invalid OIDC redirect URI: {e}"),
        ),
    };
    let client = CoreClient::from_provider_metadata(
        (*rt.metadata).clone(),
        ClientId::new(rt.config.client_id.clone()),
        Some(ClientSecret::new(rt.config.client_secret.clone())),
    )
    .set_redirect_uri(redirect);
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf, nonce) = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();
    let next = q.next.filter(|s| s.starts_with('/'));
    {
        let mut pending = rt.pending.lock().await;
        sweep_expired(&mut pending);
        pending.insert(
            csrf.secret().to_owned(),
            PendingLogin {
                nonce,
                pkce_verifier,
                next,
                created_at: Instant::now(),
            },
        );
    }
    Redirect::temporary(auth_url.as_str()).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// `/auth/callback`: exchange the authorization code, validate the
/// ID token against the nonce we stashed at /login, mint a signed
/// session cookie carrying the email claim, and 302 the user to
/// their original destination (or `/` if none was provided).
pub async fn callback(
    State(rt): State<OidcRuntime>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if let Some(err) = q.error.as_deref() {
        let detail = q.error_description.as_deref().unwrap_or("");
        return error_response(
            StatusCode::BAD_REQUEST,
            format!("OIDC provider returned error: {err}{}", if detail.is_empty() { "".into() } else { format!(" ({detail})") }),
        );
    }
    let Some(code) = q.code else {
        return error_response(StatusCode::BAD_REQUEST, "missing `code` in callback");
    };
    let Some(state) = q.state else {
        return error_response(StatusCode::BAD_REQUEST, "missing `state` in callback");
    };
    let pending = {
        let mut map = rt.pending.lock().await;
        sweep_expired(&mut map);
        map.remove(&state)
    };
    let Some(pending) = pending else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "callback `state` doesn't match a known login (expired or replayed)",
        );
    };
    let redirect = match RedirectUrl::new(rt.config.redirect_uri.clone()) {
        Ok(r) => r,
        Err(e) => return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("invalid OIDC redirect URI: {e}"),
        ),
    };
    let client = CoreClient::from_provider_metadata(
        (*rt.metadata).clone(),
        ClientId::new(rt.config.client_id.clone()),
        Some(ClientSecret::new(rt.config.client_secret.clone())),
    )
    .set_redirect_uri(redirect);
    let exchange = match client.exchange_code(AuthorizationCode::new(code)) {
        Ok(req) => req,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("provider metadata is missing the token endpoint: {e}"),
            );
        }
    };
    let token_response = match exchange
        .set_pkce_verifier(pending.pkce_verifier)
        .request_async(&rt.http)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                format!("token exchange failed: {e}"),
            );
        }
    };
    let id_token = match token_response.id_token() {
        Some(t) => t,
        None => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "OIDC provider returned no id_token",
            );
        }
    };
    let id_verifier = client.id_token_verifier();
    let claims = match id_token.claims(&id_verifier, &pending.nonce) {
        Ok(c) => c,
        Err(e) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                format!("ID token validation failed: {e}"),
            );
        }
    };
    let email = match claims.email() {
        Some(e) => e.as_str().to_string(),
        None => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "ID token has no email claim (scope `email` not granted by the IdP?)",
            );
        }
    };
    let expires_at = chrono::Utc::now()
        .timestamp()
        .saturating_add(rt.config.session_seconds);
    let cookie_value = sign(
        &rt.config.session_secret,
        &SessionPayload {
            author: email,
            expires_at,
        },
    );
    let cookie_header = format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}",
        name = SESSION_COOKIE,
        value = cookie_value,
        max_age = rt.config.session_seconds,
    );
    let next = pending.next.unwrap_or_else(|| "/".to_string());
    let mut resp = Redirect::temporary(&next).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie_header).expect("cookie header is ASCII"),
    );
    resp
}

/// `/auth/logout`: clear the cookie. We don't call the IdP's
/// end-session endpoint — single-logout is conceptually fraught
/// (the IdP session may serve other apps the user expects to stay
/// logged in to) and we can revisit if it becomes a real ask.
pub async fn logout() -> Response {
    let header_val = format!(
        "{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        name = SESSION_COOKIE,
    );
    let mut resp = (
        StatusCode::OK,
        Json(serde_json::json!({ "logged_out": true })),
    )
        .into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&header_val).expect("cookie header is ASCII"),
    );
    resp
}

fn error_response(status: StatusCode, body: impl Into<String>) -> Response {
    (status, body.into()).into_response()
}

fn sweep_expired(map: &mut HashMap<String, PendingLogin>) {
    let now = Instant::now();
    map.retain(|_, entry| now.duration_since(entry.created_at) < MAX_AGE);
}
