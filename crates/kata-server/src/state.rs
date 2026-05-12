use std::sync::Arc;

use kata_core::Author;

use crate::service::ReviewService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<ReviewService>,
    /// Identity used for writes when the client doesn't override it.
    pub default_author: Author,
}
