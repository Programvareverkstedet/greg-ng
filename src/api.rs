use std::sync::Arc;
use tokio::sync::Mutex;

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use mpvipc::{
    Mpv, NumberChangeOptions, Playlist, PlaylistAddOptions, PlaylistAddTypeOptions, SeekOptions,
    Switch,
};
use serde::Deserialize;
use serde_json::{json, Value};

type Result<T, E = crate::app_error::AppError> = std::result::Result<T, E>;

pub fn api_routes(mpv: Mpv) -> Router {
    Router::new()
        .route("/", get(index))
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
        .route("/playlist/remove", delete(playlist_remove_or_clear))
        .route("/playlist/move", post(playlist_goto))
        .route("/playlist/shuffle", post(shuffle))
        .route("/playlist/loop", get(playlist_get_looping))
        .route("/playlist/loop", post(playlist_set_looping))
        .with_state(Arc::new(Mutex::new(mpv)))
}

async fn index() -> &'static str {
    "Hello friend, I hope you're having a lovely day"
}

#[derive(Debug, Deserialize)]
struct APIRequestLoadFile {
    // Link to the resource to enqueue
    path: String,
}

/// Add item to playlist
async fn loadfile(
    State(mpv): State<Arc<Mutex<Mpv>>>,
    Query(request): Query<APIRequestLoadFile>,
) -> Result<impl IntoResponse> {
    log::trace!("POST /load {:?}", request);

    mpv.lock().await.playlist_add(
        request.path.as_str(),
        PlaylistAddTypeOptions::File,
        PlaylistAddOptions::Append,
    )?;

    Ok(Json(json!({
      "status": "true".to_string(),
      "error": false,
    })))
}

/// Check whether the player is paused or playing
async fn play_get(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("GET /play");

    let paused: bool = mpv.lock().await.get_property("pause")?;
    Ok(Json(json!({
      "value": paused,
      "error": false,
    })))
}

#[derive(Debug, Deserialize)]
struct APIRequestPlay {
    value: bool,
}

/// Set whether the player is paused or playing
async fn play_set(
    State(mpv): State<Arc<Mutex<Mpv>>>,
    Query(request): Query<APIRequestPlay>,
) -> Result<impl IntoResponse> {
    log::trace!("POST /play {:?}", request);

    mpv.lock().await.set_property("pause", request.value)?;

    Ok(Json(json!({
      "error": false,
    })))
}

/// Get the current player volume
async fn volume_get(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("GET /volume");

    let volume: f64 = mpv.lock().await.get_property("volume")?;

    Ok(Json(json!({
      "value": volume,
      "error": false,
    })))
}

#[derive(Debug, Deserialize)]
struct APIRequestVolume {
    value: f64,
}

/// Set the player volume
async fn volume_set(
    State(mpv): State<Arc<Mutex<Mpv>>>,
    Query(request): Query<APIRequestVolume>,
) -> Result<impl IntoResponse> {
    log::trace!("POST /volume {:?}", request);

    mpv.lock()
        .await
        .set_volume(request.value, NumberChangeOptions::Absolute)?;

    Ok(Json(json!({
      "error": false,
    })))
}

/// Get current playback position
async fn time_get(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("GET /time");

    let current: f64 = mpv.lock().await.get_property("time-pos")?;
    let remaining: f64 = mpv.lock().await.get_property("time-remaining")?;
    let total = current + remaining;

    Ok(Json(json!({
      "value": {
        "current": current,
        "remaining": remaining,
        "total": total,
      },
      "error": false,
    })))
}

#[derive(Debug, Deserialize)]
struct APIRequestTime {
    pos: Option<f64>,
    percent: Option<f64>,
}

/// Set playback position
async fn time_set(
    State(mpv): State<Arc<Mutex<Mpv>>>,
    Query(request): Query<APIRequestTime>,
) -> Result<impl IntoResponse> {
    log::trace!("POST /time {:?}", request);

    if request.pos.is_some() && request.percent.is_some() {
        return Err(crate::app_error::AppError(anyhow::anyhow!(
            "pos and percent cannot be provided at the same time"
        )));
    }

    if let Some(pos) = request.pos {
        mpv.lock().await.seek(pos, SeekOptions::Absolute)?;
    } else if let Some(percent) = request.percent {
        mpv.lock()
            .await
            .seek(percent, SeekOptions::AbsolutePercent)?;
    } else {
        return Err(crate::app_error::AppError(anyhow::anyhow!(
            "Either pos or percent must be provided"
        )));
    };

    Ok(Json(json!({
      "error": false,
    })))
}

/// Get the current playlist
async fn playlist_get(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("GET /playlist");

    let playlist: Playlist = mpv.lock().await.get_playlist()?;
    let is_playing: bool = mpv.lock().await.get_property("pause")?;

    let items: Vec<Value> = playlist
        .0
        .iter()
        .enumerate()
        .map(|(i, item)| {
            json!({
              "index": i,
              "current": item.current,
              "playing": is_playing,
              "filename": item.filename,
              "data": {
                "fetching": true,
              }
            })
        })
        .collect();

    Ok(Json(json!({
      "value": items,
      "error": false,
    })))
}

/// Skip to the next item in the playlist
async fn playlist_next(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("POST /playlist/next");

    Ok(Json(json!({
      "status": mpv.lock().await.next().is_ok().to_string(),
      "error": false,
    })))
}

/// Go back to the previous item in the playlist
async fn playlist_previous(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("POST /playlist/previous");

    Ok(Json(json!({
      "status": mpv.lock().await.prev().is_ok().to_string(),
      "error": false,
    })))
}

#[derive(Debug, Deserialize)]
struct APIRequestPlaylistGoto {
    index: usize,
}

/// Go chosen item in the playlist
async fn playlist_goto(
    State(mpv): State<Arc<Mutex<Mpv>>>,
    Query(request): Query<APIRequestPlaylistGoto>,
) -> Result<impl IntoResponse> {
    log::trace!("POST /playlist/goto {:?}", request);

    Ok(Json(json!({
      "status": mpv.lock().await.playlist_play_id(request.index).is_ok().to_string(),
      "error": false,
    })))
}

/// Clears single item or whole playlist
async fn playlist_remove_or_clear(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("DELETE /playlist/remove");

    Ok(Json(json!({
      "status": mpv.lock().await.playlist_clear().is_ok().to_string(),
      "error": false,
    })))
}

/// Shuffle the playlist
async fn shuffle(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("POST /playlist/shuffle");

    Ok(Json(json!({
      "status": mpv.lock().await.playlist_shuffle().is_ok().to_string(),
      "error": false,
    })))
}

/// See whether it loops the playlist or not
async fn playlist_get_looping(State(mpv): State<Arc<Mutex<Mpv>>>) -> Result<impl IntoResponse> {
    log::trace!("GET /playlist/loop");

    // TODO: this needs to be updated in the next version of the API
    // let loop_file: bool = mpv.lock().await.get_property("loop-file").unwrap();
    let loop_playlist: bool = mpv.lock().await.get_property("loop-playlist")?;

    Ok(Json(json!({
      "value": loop_playlist,
      "error": false,
    })))
}

#[derive(Debug, Deserialize)]
struct APIRequestPlaylistSetLooping {
    r#loop: bool,
}

async fn playlist_set_looping(
    State(mpv): State<Arc<Mutex<Mpv>>>,
    Query(request): Query<APIRequestPlaylistSetLooping>,
) -> Result<impl IntoResponse> {
    log::trace!("POST /playlist/loop {:?}", request);

    if request.r#loop {
        mpv.lock().await.set_loop_playlist(Switch::On)?;
    } else {
        mpv.lock().await.set_loop_playlist(Switch::Off)?;
    }

    Ok(Json(json!({
      "status": request.r#loop.to_string(),
      "error": false,
    })))
}
