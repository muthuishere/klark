//! `feed.list | stop | start` (ADR 0015, M5). Each reads or writes the feed
//! store and may wait for a worker, so they run on their own thread, never
//! on the app loop; the reply goes to `respond_to`.

use crate::api::schema::{Method, Request, ResponseResult};
use crate::app::App;

use super::responses::{encode_error, encode_success};

impl App {
    /// `true` when `request` is a feed method; it is then answered on
    /// `respond_to` from a thread of its own.
    pub(crate) fn handle_deferred_feed_api_request(
        request: Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        if !matches!(
            request.method,
            Method::FeedList(_) | Method::FeedStop(_) | Method::FeedStart(_)
        ) {
            return false;
        }
        let failed = respond_to.clone();
        let id = request.id.clone();
        let spawned = crate::thread_spawn::spawn_named("klark-feed-api", move || {
            let _ = respond_to.send(run_feed_request(request));
        });
        if let Err(err) = spawned {
            let _ = failed.send(encode_error(id, "server_unavailable", err.to_string()));
        }
        true
    }
}

fn run_feed_request(request: Request) -> String {
    let id = request.id;
    let result = match request.method {
        Method::FeedList(_) => {
            crate::feed_cmd::api_list().map(|feeds| ResponseResult::FeedList { feeds })
        }
        Method::FeedStop(target) => crate::feed_cmd::api_stop(&target.id)
            .map(|()| ResponseResult::FeedStopped { id: target.id }),
        Method::FeedStart(target) => {
            crate::feed_cmd::api_start(&target.id).map(|state| ResponseResult::FeedStarted {
                id: target.id,
                state,
            })
        }
        _ => Err("not a feed method".into()),
    };
    match result {
        Ok(result) => encode_success(id, result),
        Err(message) => encode_error(id, "feed_failed", message),
    }
}
