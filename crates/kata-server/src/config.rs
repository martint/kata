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

#[derive(Clone, Debug)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}
