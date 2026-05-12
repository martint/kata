//! Pub/sub for public state changes. Transports (HTTP/SSE, MCP) subscribe
//! to be notified when something a viewer cares about changes — currently:
//! a new review, a refreshed review, a published session, a discarded
//! session. Drafts are intentionally excluded; they're only visible to
//! their author.

use kata_core::{ReviewId, SessionId};
use serde::Serialize;
use tokio::sync::broadcast;

/// What changed in the world that other viewers should know about.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Event {
    ReviewCreated {
        repo: String,
        review_id: ReviewId,
    },
    ReviewUpdated {
        repo: String,
        review_id: ReviewId,
    },
    SessionPublished {
        repo: String,
        review_id: ReviewId,
        session_id: SessionId,
    },
    SessionDiscarded {
        repo: String,
        review_id: ReviewId,
        session_id: SessionId,
    },
}

pub type EventBus = broadcast::Sender<Event>;

pub fn new_bus() -> EventBus {
    // Buffer is generous because we never want a slow subscriber to make
    // the publisher block; the dropped-event signal on the receiver side
    // (BroadcastStreamRecvError::Lagged) is acceptable for our use case.
    let (tx, _rx) = broadcast::channel(256);
    tx
}
