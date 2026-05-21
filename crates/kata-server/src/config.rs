use std::net::SocketAddr;
use std::path::PathBuf;

use kata_core::Author;

use crate::auth::AuthConfig;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub review_root: PathBuf,
    pub author: Author,
    pub bind_addr: SocketAddr,
    pub auth: AuthConfig,
    pub tls: Option<TlsConfig>,
}

/// How the server should terminate TLS, when any TLS at all is asked
/// for. The two arms are mutually exclusive at the CLI; the
/// validator in `parse_args` rejects a config that sets both.
#[derive(Clone, Debug)]
pub enum TlsConfig {
    /// Operator-supplied cert + key on disk. Kata loads them once at
    /// startup; refreshing them is the operator's job (cert-bot,
    /// external ACME script, etc.) plus a server restart.
    Pem {
        cert_path: PathBuf,
        key_path: PathBuf,
    },
    /// Auto-issued certificate via ACME. Kata speaks the ACME
    /// protocol itself (via `rustls-acme`) on the same 443 listener
    /// as the application — TLS-ALPN-01 challenge, no extra port
    /// needed. The account key and issued cert are persisted under
    /// `cache_dir` so a restart doesn't re-run the dance.
    Acme {
        /// FQDN to issue for. Must point at this server's bind
        /// address by the time the first request lands.
        domain: String,
        /// On-disk cache. Holds the ACME account key and issued
        /// cert / chain. Lost cache = re-issuance + a 5-per-week
        /// Let's Encrypt rate-limit ding, so don't lose it.
        cache_dir: PathBuf,
        /// Optional `mailto:` contact handed to the CA. Let's
        /// Encrypt uses it for expiry reminders. Strongly
        /// recommended but not required.
        contact: Option<String>,
        /// When `true`, use Let's Encrypt's staging endpoint
        /// (untrusted CA, no rate limits) — useful for testing
        /// without burning weekly issuance budget.
        staging: bool,
    },
}
