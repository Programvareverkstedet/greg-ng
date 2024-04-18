use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use mpvipc_async::Mpv;
use serde_json::{json, Value};

use super::base;

pub fn rest_api_routes(mpv: Mpv) -> Router {
    Router::new()
        .route("/load", post(loadfile))
        .route("/play", get(play_get))
        .route("/play", post(play_set))
        .route("/volume", get(volume_get))
        .route("/volume", post(volume_set))
        .route("/time", get(time_get))
        .route("/time", post(time_set))
        .route("/playlist", get(playlist_get))
        .route("/playlist/next", post(playlist_next))
        .route("/playlist/previous", post(playlist_previous))
        .route("/playlist/goto", post(playlist_goto))
        .route("/playlist", delete(playlist_remove_or_clear))
        .route("/playlist/move", post(playlist_move))
        .route("/playlist/shuffle", post(shuffle))
        .route("/playlist/loop", get(playlist_get_looping))
        .route("/playlist/loop", post(playlist_set_looping))
        .with_state(mpv)
}

pub struct RestResponse(anyhow::Result<Value>);

impl From<anyhow::Result<Value>> for RestResponse {
    fn from(result: anyhow::Result<Value>) -> Self {
        Self(result.map(|value| json!({ "success": true, "error": false, "value": value })))
    }
}

impl From<anyhow::Result<()>> for RestResponse {
    fn from(result: anyhow::Result<()>) -> Self {
        Self(result.map(|_| json!({ "success": true, "error": false })))
    }
}

impl IntoResponse for RestResponse {
    fn into_response(self) -> Response {
        match self.0 {
            Ok(value) => (StatusCode::OK, Json(value)).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string(), "errortext": err.to_string(), "success": false })),
            )
                .into_response(),
        }
    }
}

// -------------------//
// Boilerplate galore //
// -------------------//

// TODO: These could possibly be generated with a proc macro

#[derive(serde::Deserialize)]
struct LoadFileArgs {
    path: String,
}

async fn loadfile(State(mpv): State<Mpv>, Query(query): Query<LoadFileArgs>) -> RestResponse {
    base::loadfile(mpv, &query.path).await.into()
}

async fn play_get(State(mpv): State<Mpv>) -> RestResponse {
    base::play_get(mpv).await.into()
}

#[derive(serde::Deserialize)]
struct PlaySetArgs {
    play: String,
}

async fn play_set(State(mpv): State<Mpv>, Query(query): Query<PlaySetArgs>) -> RestResponse {
    let play = query.play.to_lowercase() == "true";
    base::play_set(mpv, play).await.into()
}

async fn volume_get(State(mpv): State<Mpv>) -> RestResponse {
    base::volume_get(mpv).await.into()
}

#[derive(serde::Deserialize)]
struct VolumeSetArgs {
    volume: f64,
}

async fn volume_set(State(mpv): State<Mpv>, Query(query): Query<VolumeSetArgs>) -> RestResponse {
    base::volume_set(mpv, query.volume).await.into()
}

async fn time_get(State(mpv): State<Mpv>) -> RestResponse {
    base::time_get(mpv).await.into()
}

#[derive(serde::Deserialize)]
struct TimeSetArgs {
    pos: Option<f64>,
    percent: Option<f64>,
}

async fn time_set(State(mpv): State<Mpv>, Query(query): Query<TimeSetArgs>) -> RestResponse {
    base::time_set(mpv, query.pos, query.percent).await.into()
}

async fn playlist_get(State(mpv): State<Mpv>) -> RestResponse {
    base::playlist_get(mpv).await.into()
}

async fn playlist_next(State(mpv): State<Mpv>) -> RestResponse {
    base::playlist_next(mpv).await.into()
}

async fn playlist_previous(State(mpv): State<Mpv>) -> RestResponse {
    base::playlist_previous(mpv).await.into()
}

#[derive(serde::Deserialize)]
struct PlaylistGotoArgs {
    index: usize,
}

async fn playlist_goto(
    State(mpv): State<Mpv>,
    Query(query): Query<PlaylistGotoArgs>,
) -> RestResponse {
    base::playlist_goto(mpv, query.index).await.into()
}

#[derive(serde::Deserialize)]
struct PlaylistRemoveOrClearArgs {
    index: Option<usize>,
}

async fn playlist_remove_or_clear(
    State(mpv): State<Mpv>,
    Query(query): Query<PlaylistRemoveOrClearArgs>,
) -> RestResponse {
    match query.index {
        Some(index) => base::playlist_remove(mpv, index).await.into(),
        None => base::playlist_clear(mpv).await.into(),
    }
}

#[derive(serde::Deserialize)]
struct PlaylistMoveArgs {
    index1: usize,
    index2: usize,
}

async fn playlist_move(
    State(mpv): State<Mpv>,
    Query(query): Query<PlaylistMoveArgs>,
) -> RestResponse {
    base::playlist_move(mpv, query.index1, query.index2)
        .await
        .into()
}

async fn shuffle(State(mpv): State<Mpv>) -> RestResponse {
    base::shuffle(mpv).await.into()
}

async fn playlist_get_looping(State(mpv): State<Mpv>) -> RestResponse {
    base::playlist_get_looping(mpv).await.into()
}

#[derive(serde::Deserialize)]
struct PlaylistSetLoopingArgs {
    r#loop: bool,
}

async fn playlist_set_looping(
    State(mpv): State<Mpv>,
    Query(query): Query<PlaylistSetLoopingArgs>,
) -> RestResponse {
    base::playlist_set_looping(mpv, query.r#loop).await.into()
}
