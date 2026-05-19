use std::net::SocketAddr;
use std::sync::Arc;

use kata_core::Author;

use crate::auth::AuthConfig;
use crate::service::ReviewService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<ReviewService>,
    /// Identity used for writes when the client doesn't override it.
    /// Only consulted in `AuthMode::TrustClient`; in
    /// `TrustForwardedHeader` mode a missing trusted header is a 401,
    /// never a fall-through to this default.
    pub default_author: Author,
    pub auth: AuthConfig,
    /// The socket the server is bound on. Threaded through so the
    /// auth middleware can short-circuit the upstream-IP check on
    /// loopback binds.
    pub bind_addr: SocketAddr,
}
