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
    /// The review row and all its sessions/comments/responses/
    /// annotations/visits were deleted. Open viewers should drop
    /// state and return to the list.
    ReviewDeleted {
        repo: String,
        review_id: ReviewId,
    },
    /// The underlying branch moved relative to the review's latest
    /// patchset — refreshing the review would advance it. Emitted by
    /// the background watcher (see [`crate::ReviewService::spawn_branch_watcher`]).
    ReviewBranchMoved {
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
    /// A workspace was just registered (operator dropped a repo into
    /// a scanned base, or a dynamic `add_repo` call). The repo list
    /// in any open tab should refresh so the new slug is selectable.
    WorkspaceRegistered {
        repo: String,
    },
    /// A registered workspace was unregistered (scanned dir gone, or
    /// a dynamic `remove_repo` call). Open tabs viewing that repo
    /// should drop back to the home / list screen.
    WorkspaceUnregistered {
        repo: String,
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
