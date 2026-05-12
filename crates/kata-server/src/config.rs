use std::net::SocketAddr;
use std::path::PathBuf;

use kata_core::Author;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub review_root: PathBuf,
    pub author: Author,
    pub bind_addr: SocketAddr,
}
