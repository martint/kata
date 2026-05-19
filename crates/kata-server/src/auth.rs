//! Auth configuration for the HTTP and MCP transports.
//!
//! Kata has historically trusted the client to set its own identity
//! via `X-Review-Author` (HTTP) or `?as=` (MCP). That's safe on a
//! localhost dev box and a soft hole on anything shared, because any
//! caller can claim to be anyone. This module introduces a second
//! mode — `trust-forwarded-header` — where the server reads the actor
//! from a header set by an upstream proxy that's done the actual
//! authentication, and ignores client-supplied identity entirely.
//!
//! The chosen mode + ancillary settings ride along inside [`AppState`]
//! so the per-request extractor (`routes::author::ViewerAuthor`) and
//! the MCP handler can both honour the same configuration.

use std::net::IpAddr;
use std::str::FromStr;

use ipnet::IpNet;

/// How the server decides who is acting on a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum AuthMode {
    /// Read the actor from a client-supplied header (`X-Review-Author`
    /// on HTTP, `?as=` on MCP) — the historical behaviour. Safe on a
    /// single-user / localhost setup, unsafe for anything shared: any
    /// caller can claim any identity. Falls back to the server's
    /// configured default author when the client supplies nothing.
    TrustClient,
    /// Read the actor from a header an upstream proxy is responsible
    /// for setting (`--auth-trusted-header`, default `X-Forwarded-
    /// Email`). The proxy is in charge of authenticating the user
    /// (typically via OIDC) and signing the actor onto the request;
    /// Kata trusts whatever the proxy says, and ignores client-
    /// supplied identity headers. A request that reaches Kata without
    /// the trusted header gets a 401 — never the default author —
    /// because in this mode the absence of a header means the proxy
    /// failed to authenticate.
    TrustForwardedHeader,
}

/// Configuration that the auth path consults on every request.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub mode: AuthMode,
    /// The header to read the actor from in
    /// [`AuthMode::TrustForwardedHeader`]. Lower-case is fine — HTTP
    /// header names are case-insensitive and axum normalises on the
    /// way in.
    pub trusted_header: String,
    /// CIDR ranges allowed to set the trusted header. Empty means
    /// "no remote origin is trusted" — the server refuses to start
    /// in `trust-forwarded-header` mode on a non-loopback bind unless
    /// at least one network is in the allowlist. Loopback binds
    /// don't consult this list (the only thing that can connect to
    /// loopback is the same host).
    pub upstream_allowlist: Vec<IpNet>,
}

impl AuthConfig {
    /// Whether the per-request middleware should enforce the
    /// upstream-IP allowlist. The check is only meaningful for
    /// `trust-forwarded-header` mode on a non-loopback bind — both
    /// because trust-client doesn't honour the trusted header at all
    /// and because nothing but the same host can reach a loopback
    /// listener.
    pub fn enforce_allowlist(&self, bind: std::net::SocketAddr) -> bool {
        self.mode == AuthMode::TrustForwardedHeader && !bind.ip().is_loopback()
    }

    /// True iff `remote` is allowed to set the trusted header on a
    /// non-loopback bind. Loopback connections (somehow reaching here
    /// despite a non-loopback bind — e.g. a same-host curl) are
    /// always allowed: a process on the host has full control of the
    /// machine anyway, so blocking it offers no real protection.
    pub fn upstream_allowed(&self, remote: IpAddr) -> bool {
        if remote.is_loopback() {
            return true;
        }
        self.upstream_allowlist.iter().any(|net| net.contains(&remote))
    }
}

/// Reason `validate_bind_safety` refused to start. Carried as its
/// own error so callers can format the message however they like
/// (the CLI uses the `Display` impl verbatim; tests pattern-match).
#[derive(Debug, thiserror::Error)]
pub enum BindSafetyError {
    /// `trust-forwarded-header` on a non-loopback bind without a
    /// configured upstream allowlist. Refusing here keeps a
    /// misconfigured deployment from silently honouring the trusted
    /// header for any caller on the network.
    #[error(
        "--auth-mode trust-forwarded-header on a non-loopback bind ({bind}) requires \
         at least one --auth-trust-upstream <cidr> to identify the proxy. \
         Pass `--auth-trust-upstream 0.0.0.0/0` if you really mean to trust any source \
         (only sensible inside an isolated network)."
    )]
    ForwardedHeaderWithoutAllowlist { bind: std::net::SocketAddr },
}

/// Refuse to start when the auth-mode / bind combination is unsafe.
///
/// `trust-forwarded-header` is meaningful only when there's a proxy
/// in front. We can't actually verify the proxy is there, so we
/// require evidence — either the bind is loopback (only same-host
/// processes can reach us anyway) or the operator has set an
/// upstream allowlist. Without either, the trusted header would be
/// honoured from any client on the network, which is the exact
/// foot-gun the mode exists to prevent.
pub fn validate_bind_safety(
    bind: std::net::SocketAddr,
    auth: &AuthConfig,
) -> Result<(), BindSafetyError> {
    if auth.mode == AuthMode::TrustForwardedHeader
        && !bind.ip().is_loopback()
        && auth.upstream_allowlist.is_empty()
    {
        return Err(BindSafetyError::ForwardedHeaderWithoutAllowlist { bind });
    }
    Ok(())
}

/// Parse a `--auth-trust-upstream` flag value: either `0.0.0.0/0`
/// (full open), a bare IP (`/32` or `/128` implied), or any standard
/// CIDR notation. Used by clap's `value_parser`.
pub fn parse_upstream_cidr(s: &str) -> Result<IpNet, String> {
    let trimmed = s.trim();
    // Bare IP — treat as a host-only network (single address).
    if let Ok(addr) = IpAddr::from_str(trimmed) {
        let prefix = if addr.is_ipv4() { 32 } else { 128 };
        return IpNet::new(addr, prefix)
            .map_err(|e| format!("invalid host CIDR {trimmed:?}: {e}"));
    }
    IpNet::from_str(trimmed).map_err(|e| format!("invalid CIDR {trimmed:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    fn cfg(mode: AuthMode, allow: &[&str]) -> AuthConfig {
        AuthConfig {
            mode,
            trusted_header: "x-forwarded-email".into(),
            upstream_allowlist: allow
                .iter()
                .map(|s| parse_upstream_cidr(s).unwrap())
                .collect(),
        }
    }

    #[test]
    fn validate_rejects_forwarded_header_on_non_loopback_without_allowlist() {
        let auth = cfg(AuthMode::TrustForwardedHeader, &[]);
        let bind: SocketAddr = "0.0.0.0:7878".parse().unwrap();
        let err = validate_bind_safety(bind, &auth).unwrap_err();
        assert!(
            matches!(err, BindSafetyError::ForwardedHeaderWithoutAllowlist { .. }),
            "expected ForwardedHeaderWithoutAllowlist, got {err:?}",
        );
    }

    #[test]
    fn validate_allows_forwarded_header_on_loopback_without_allowlist() {
        let auth = cfg(AuthMode::TrustForwardedHeader, &[]);
        let bind: SocketAddr = "127.0.0.1:7878".parse().unwrap();
        validate_bind_safety(bind, &auth).unwrap();
    }

    #[test]
    fn validate_allows_forwarded_header_with_allowlist() {
        let auth = cfg(AuthMode::TrustForwardedHeader, &["10.0.0.0/8"]);
        let bind: SocketAddr = "0.0.0.0:7878".parse().unwrap();
        validate_bind_safety(bind, &auth).unwrap();
    }

    #[test]
    fn validate_allows_trust_client_anywhere() {
        let auth = cfg(AuthMode::TrustClient, &[]);
        validate_bind_safety("0.0.0.0:7878".parse().unwrap(), &auth).unwrap();
        validate_bind_safety("127.0.0.1:7878".parse().unwrap(), &auth).unwrap();
    }

    #[test]
    fn loopback_is_always_allowed_even_with_empty_allowlist() {
        let c = cfg(AuthMode::TrustForwardedHeader, &[]);
        assert!(c.upstream_allowed(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(c.upstream_allowed(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn non_loopback_blocked_by_empty_allowlist() {
        let c = cfg(AuthMode::TrustForwardedHeader, &[]);
        assert!(!c.upstream_allowed(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn cidr_match_allows_inside_range() {
        let c = cfg(AuthMode::TrustForwardedHeader, &["10.0.0.0/8"]);
        assert!(c.upstream_allowed(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(!c.upstream_allowed(IpAddr::V4(Ipv4Addr::new(11, 0, 0, 1))));
    }

    #[test]
    fn bare_ip_parses_to_host_cidr() {
        let net = parse_upstream_cidr("192.168.1.5").unwrap();
        assert_eq!(net.prefix_len(), 32);
        assert!(net.contains(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5))));
        assert!(!net.contains(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 6))));
    }

    #[test]
    fn allowlist_only_consulted_for_forwarded_mode_on_non_loopback() {
        let bind: std::net::SocketAddr = "0.0.0.0:7878".parse().unwrap();
        let loop_bind: std::net::SocketAddr = "127.0.0.1:7878".parse().unwrap();
        assert!(cfg(AuthMode::TrustForwardedHeader, &[]).enforce_allowlist(bind));
        assert!(!cfg(AuthMode::TrustForwardedHeader, &[]).enforce_allowlist(loop_bind));
        assert!(!cfg(AuthMode::TrustClient, &[]).enforce_allowlist(bind));
    }
}
