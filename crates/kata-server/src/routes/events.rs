use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::state::AppState;

/// Subscribe to the service's event bus and stream events as Server-Sent
/// Events. Each message body is a JSON-encoded
/// [`kata_service::Event`]. A keep-alive comment goes out every 15s so
/// proxies don't close the connection on long-idle reviews.
pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.service.events().subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|res| {
        // Drop "lagged" errors silently — a refreshing client will catch up.
        let event = res.ok()?;
        let data = serde_json::to_string(&event).ok()?;
        Some(Ok(SseEvent::default().data(data)))
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}
