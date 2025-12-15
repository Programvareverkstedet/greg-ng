use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use mpvipc_async::Mpv;
use serde_json::{Value, json};

use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_swagger_ui::SwaggerUi;

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

pub fn rest_api_docs(mpv: Mpv) -> Router {
    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(loadfile))
        .routes(routes!(play_get, play_set))
        .routes(routes!(volume_get, volume_set))
        .routes(routes!(time_get, time_set))
        .routes(routes!(playlist_get, playlist_remove_or_clear))
        .routes(routes!(playlist_next))
        .routes(routes!(playlist_previous))
        .routes(routes!(playlist_goto))
        .routes(routes!(playlist_move))
        .routes(routes!(playlist_get_looping, playlist_set_looping))
        .routes(routes!(shuffle))
        .with_state(mpv)
        .split_for_parts();

    router.merge(SwaggerUi::new("/docs").url("/docs/openapi.json", api))
}

// NOTE: the openapi stuff is very heavily duplicated and introduces
//       a lot of maintenance overhead and boilerplate. It should theoretically
//       be possible to infer a lot of this from axum, but I haven't found a
//       good library that does this and works properly yet (I have tried some
//       but they all had issues). Feel free to replace this with a better solution.

#[derive(OpenApi)]
#[openapi(info(
    description = "The legacy Grzegorz Brzeczyszczykiewicz API, used to control a running mpv instance",
    version = "1.0.0",
))]
struct ApiDoc;

#[derive(serde::Serialize, utoipa::ToSchema)]
struct EmptySuccessResponse {
    success: bool,
    error: bool,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
struct SuccessResponse {
    #[schema(example = true)]
    success: bool,
    #[schema(example = false)]
    error: bool,
    #[schema(example = json!({ some: "arbitrary json value" }))]
    value: Value,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
struct ErrorResponse {
    #[schema(example = "error....")]
    error: String,
    #[schema(example = "error....")]
    errortext: String,
    #[schema(example = false)]
    success: bool,
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

#[derive(serde::Deserialize, utoipa::IntoParams)]
struct LoadFileArgs {
    path: String,
}

/// Add item to playlist
#[utoipa::path(
    post,
    path = "/load",
    params(LoadFileArgs),
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn loadfile(State(mpv): State<Mpv>, Query(query): Query<LoadFileArgs>) -> RestResponse {
    base::loadfile(mpv, &query.path).await.into()
}

/// Check whether the player is paused or playing
#[utoipa::path(
    get,
    path = "/play",
    responses(
        (status = 200, description = "Success", body = SuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn play_get(State(mpv): State<Mpv>) -> RestResponse {
    base::play_get(mpv).await.into()
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
struct PlaySetArgs {
    play: String,
}

/// Set whether the player is paused or playing
#[utoipa::path(
    post,
    path = "/play",
    params(PlaySetArgs),
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn play_set(State(mpv): State<Mpv>, Query(query): Query<PlaySetArgs>) -> RestResponse {
    let play = query.play.to_lowercase() == "true";
    base::play_set(mpv, play).await.into()
}

/// Get the current player volume
#[utoipa::path(
    get,
    path = "/volume",
    responses(
        (status = 200, description = "Success", body = SuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn volume_get(State(mpv): State<Mpv>) -> RestResponse {
    base::volume_get(mpv).await.into()
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
struct VolumeSetArgs {
    volume: f64,
}

/// Set the player volume
#[utoipa::path(
    post,
    path = "/volume",
    params(VolumeSetArgs),
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn volume_set(State(mpv): State<Mpv>, Query(query): Query<VolumeSetArgs>) -> RestResponse {
    base::volume_set(mpv, query.volume).await.into()
}

/// Get current playback position
#[utoipa::path(
    get,
    path = "/time",
    responses(
        (status = 200, description = "Success", body = SuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn time_get(State(mpv): State<Mpv>) -> RestResponse {
    base::time_get(mpv).await.into()
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
struct TimeSetArgs {
    pos: Option<f64>,
    percent: Option<f64>,
}

/// Set playback position
#[utoipa::path(
    post,
    path = "/time",
    params(TimeSetArgs),
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn time_set(State(mpv): State<Mpv>, Query(query): Query<TimeSetArgs>) -> RestResponse {
    base::time_set(mpv, query.pos, query.percent).await.into()
}

/// Get the current playlist
#[utoipa::path(
    get,
    path = "/playlist",
    responses(
        (status = 200, description = "Success", body = SuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn playlist_get(State(mpv): State<Mpv>) -> RestResponse {
    base::playlist_get(mpv).await.into()
}

/// Go to the next item in the playlist
#[utoipa::path(
    post,
    path = "/playlist/next",
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn playlist_next(State(mpv): State<Mpv>) -> RestResponse {
    base::playlist_next(mpv).await.into()
}

/// Go back to the previous item in the playlist
#[utoipa::path(
    post,
    path = "/playlist/previous",
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn playlist_previous(State(mpv): State<Mpv>) -> RestResponse {
    base::playlist_previous(mpv).await.into()
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
struct PlaylistGotoArgs {
    index: usize,
}

/// Go to a specific item in the playlist
#[utoipa::path(
    post,
    path = "/playlist/goto",
    params(PlaylistGotoArgs),
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn playlist_goto(
    State(mpv): State<Mpv>,
    Query(query): Query<PlaylistGotoArgs>,
) -> RestResponse {
    base::playlist_goto(mpv, query.index).await.into()
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
struct PlaylistRemoveOrClearArgs {
    index: Option<usize>,
}

/// Clears a single item or the entire playlist
#[utoipa::path(
    delete,
    path = "/playlist",
    params(PlaylistRemoveOrClearArgs),
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn playlist_remove_or_clear(
    State(mpv): State<Mpv>,
    Query(query): Query<PlaylistRemoveOrClearArgs>,
) -> RestResponse {
    match query.index {
        Some(index) => base::playlist_remove(mpv, index).await.into(),
        None => base::playlist_clear(mpv).await.into(),
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
struct PlaylistMoveArgs {
    index1: usize,
    index2: usize,
}

/// Move a playlist item to a different position
#[utoipa::path(
    post,
    path = "/playlist/move",
    params(PlaylistMoveArgs),
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn playlist_move(
    State(mpv): State<Mpv>,
    Query(query): Query<PlaylistMoveArgs>,
) -> RestResponse {
    base::playlist_move(mpv, query.index1, query.index2)
        .await
        .into()
}

/// Shuffle the playlist
#[utoipa::path(
    post,
    path = "/playlist/shuffle",
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn shuffle(State(mpv): State<Mpv>) -> RestResponse {
    base::shuffle(mpv).await.into()
}

/// Check whether the playlist is looping
#[utoipa::path(
    get,
    path = "/playlist/loop",
    responses(
        (status = 200, description = "Success", body = SuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn playlist_get_looping(State(mpv): State<Mpv>) -> RestResponse {
    base::playlist_get_looping(mpv).await.into()
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
struct PlaylistSetLoopingArgs {
    r#loop: bool,
}

/// Set whether the playlist should loop
#[utoipa::path(
    post,
    path = "/playlist/loop",
    params(PlaylistSetLoopingArgs),
    responses(
        (status = 200, description = "Success", body = EmptySuccessResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
async fn playlist_set_looping(
    State(mpv): State<Mpv>,
    Query(query): Query<PlaylistSetLoopingArgs>,
) -> RestResponse {
    base::playlist_set_looping(mpv, query.r#loop).await.into()
}
