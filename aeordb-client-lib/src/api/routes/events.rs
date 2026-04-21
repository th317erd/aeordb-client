use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::server::AppState;

pub async fn event_stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|result| result.ok())
        .map(|data| Ok(Event::default().event("sync_activity").data(data)));

    Sse::new(stream).keep_alive(
        KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    )
}
