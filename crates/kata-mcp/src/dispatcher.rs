//! Per-author dispatch for the MCP endpoint.
//!
//! The MCP transport itself doesn't carry caller identity, so kata reads
//! it from a `?as=<name>` query param on each `/mcp` request and routes
//! to a per-author [`StreamableHttpService`]. When the param is absent
//! the [`McpDispatcher::default_author`] is used.
//!
//! Each author gets its own service instance with its own session
//! manager — sessions for different authors are isolated. This is a
//! pre-auth stopgap so e.g. an agent like Claude can be attributed
//! distinctly from the human user without us building a real auth story.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use kata_core::Author;
use kata_service::ReviewService;
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, session::local::LocalSessionManager,
};

use crate::tools::{ReviewMcp, mcp_service};

/// Query-string key used to override the author on a per-request basis.
pub const AUTHOR_QUERY_PARAM: &str = "as";

type Instance = StreamableHttpService<ReviewMcp, LocalSessionManager>;

#[derive(Clone)]
pub struct McpDispatcher {
    inner: Arc<Inner>,
}

struct Inner {
    service: Arc<ReviewService>,
    default_author: Author,
    instances: Mutex<HashMap<String, Instance>>,
}

impl McpDispatcher {
    pub fn new(service: Arc<ReviewService>, default_author: Author) -> Self {
        Self {
            inner: Arc::new(Inner {
                service,
                default_author,
                instances: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn default_author(&self) -> &Author {
        &self.inner.default_author
    }

    /// Get (or create on first sight) the MCP service for an author name.
    /// The map is keyed by the raw string so two callers passing the
    /// same `?as=` value share a session manager.
    pub fn for_author(&self, raw: &str) -> Instance {
        let mut map = self.inner.instances.lock().expect("mcp dispatcher lock poisoned");
        map.entry(raw.to_owned())
            .or_insert_with(|| mcp_service(self.inner.service.clone(), Author::new(raw)))
            .clone()
    }
}
