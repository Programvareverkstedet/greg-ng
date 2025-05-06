use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use futures::{stream::FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};

use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::any,
    Router,
};
use mpvipc_async::{
    LoopProperty, Mpv, MpvExt, NumberChangeOptions, Playlist, PlaylistAddTypeOptions, SeekOptions,
    Switch,
};
use serde_json::{json, Value};
use tokio::{
    select,
    sync::{mpsc, watch},
};

use crate::util::{ConnectionEvent, IdPool};

#[derive(Debug, Clone)]
struct WebsocketState {
    mpv: Mpv,
    id_pool: Arc<Mutex<IdPool>>,
    connection_counter_tx: mpsc::Sender<ConnectionEvent>,
}

pub fn websocket_api(
    mpv: Mpv,
    id_pool: Arc<Mutex<IdPool>>,
    connection_counter_tx: mpsc::Sender<ConnectionEvent>,
) -> Router {
    let state = WebsocketState {
        mpv,
        id_pool,
        connection_counter_tx,
    };
    Router::new()
        .route("/", any(websocket_handler))
        .with_state(state)
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(WebsocketState {
        mpv,
        id_pool,
        connection_counter_tx,
    }): State<WebsocketState>,
) -> impl IntoResponse {
    let mpv = mpv.clone();
    let id = match id_pool.lock().unwrap().request_id() {
        Ok(id) => id,
        Err(e) => {
            log::error!("Failed to get id from id pool: {:?}", e);
            return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    ws.on_upgrade(move |socket| {
        handle_connection(socket, addr, mpv, id, id_pool, connection_counter_tx)
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitialState {
    pub cached_timestamp: Option<f64>,
    pub chapters: Vec<Value>,
    pub connections: u64,
    pub current_percent_pos: Option<f64>,
    pub current_track: String,
    pub duration: f64,
    pub is_looping: bool,
    pub is_muted: bool,
    pub is_playing: bool,
    pub is_paused_for_cache: bool,
    pub playlist: Playlist,
    pub tracks: Vec<Value>,
    pub volume: f64,
}

async fn get_initial_state(mpv: &Mpv, id_pool: Arc<Mutex<IdPool>>) -> InitialState {
    let cached_timestamp = mpv
        .get_property_value("demuxer-cache-state")
        .await
        .unwrap_or(None)
        .and_then(|v| {
            v.as_object()
                .and_then(|o| o.get("data"))
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("cache-end"))
                .and_then(|v| v.as_f64())
        });
    let chapters = match mpv.get_property_value("chapter-list").await {
        Ok(Some(Value::Array(chapters))) => chapters,
        _ => vec![],
    };
    let connections = id_pool.lock().unwrap().id_count();
    let current_percent_pos = mpv.get_property("percent-pos").await.unwrap_or(None);
    let current_track = mpv.get_file_path().await.unwrap_or("".to_string());
    let duration = mpv.get_duration().await.unwrap_or(0.0);
    let is_looping =
        mpv.playlist_is_looping().await.unwrap_or(LoopProperty::No) != LoopProperty::No;
    let is_muted = mpv
        .get_property("mute")
        .await
        .unwrap_or(Some(false))
        .unwrap_or(false);
    let is_playing = mpv.is_playing().await.unwrap_or(false);
    let is_paused_for_cache = mpv
        .get_property("paused-for-cache")
        .await
        .unwrap_or(Some(false))
        .unwrap_or(false);
    let playlist = mpv.get_playlist().await.unwrap_or(Playlist(vec![]));
    let tracks = match mpv.get_property_value("track-list").await {
        Ok(Some(Value::Array(tracks))) => tracks
            .into_iter()
            .filter(|t| {
                t.as_object()
                    .and_then(|o| o.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    == "sub"
            })
            .collect(),
        _ => vec![],
    };
    let volume = mpv.get_volume().await.unwrap_or(0.0);
    // TODO: use default when new version is released
    InitialState {
        cached_timestamp,
        chapters,
        connections,
        current_percent_pos,
        current_track,
        duration,
        is_looping,
        is_muted,
        is_playing,
        is_paused_for_cache,
        playlist,
        tracks,
        volume,
    }
}

const DEFAULT_PROPERTY_SUBSCRIPTIONS: [&str; 11] = [
    "chapter-list",
    "demuxer-cache-state",
    "duration",
    "loop-playlist",
    "mute",
    "pause",
    "paused-for-cache",
    "percent-pos",
    "playlist",
    "track-list",
    "volume",
];

async fn setup_default_subscribes(mpv: &Mpv) -> anyhow::Result<()> {
    let mut futures = FuturesUnordered::new();

    futures.extend(
        DEFAULT_PROPERTY_SUBSCRIPTIONS
            .iter()
            .map(|property| mpv.observe_property(0, property)),
    );

    while let Some(result) = futures.next().await {
        result?;
    }

    Ok(())
}

async fn handle_connection(
    mut socket: WebSocket,
    addr: SocketAddr,
    mpv: Mpv,
    channel_id: u64,
    id_pool: Arc<Mutex<IdPool>>,
    connection_counter_tx: mpsc::Sender<ConnectionEvent>,
) {
    match connection_counter_tx.send(ConnectionEvent::Connected).await {
        Ok(()) => {
            log::trace!("Connection count updated for {:?}", addr);
        }
        Err(e) => {
            log::error!("Error updating connection count for {:?}: {:?}", addr, e);
        }
    }

    // TODO: There is an asynchronous gap between gathering the initial state and subscribing to the properties
    //       This could lead to missing events if they happen in that gap. Send initial state, but also ensure
    //       that there is an additional "initial state" sent upon subscription to all properties to ensure that
    //       the state is correct.
    let initial_state = get_initial_state(&mpv, id_pool.clone()).await;

    let message = Message::Text(
        json!({
            "type": "initial_state",
            "value": initial_state,
        })
        .to_string(),
    );

    socket.send(message).await.unwrap();

    setup_default_subscribes(&mpv).await.unwrap();

    let id_count_watch_receiver = id_pool.lock().unwrap().get_id_count_watch_receiver();

    let connection_loop_result = tokio::spawn(connection_loop(
        socket,
        addr,
        mpv.clone(),
        channel_id,
        id_count_watch_receiver,
    ));

    match connection_loop_result.await {
        Ok(Ok(())) => {
            log::trace!("Connection loop ended for {:?}", addr);
        }
        Ok(Err(e)) => {
            log::error!("Error in connection loop for {:?}: {:?}", addr, e);
        }
        Err(e) => {
            log::error!("Error in connection loop for {:?}: {:?}", addr, e);
        }
    }

    match mpv.unobserve_property(channel_id).await {
        Ok(()) => {
            log::trace!("Unsubscribed from properties for {:?}", addr);
        }
        Err(e) => {
            log::error!(
                "Error unsubscribing from properties for {:?}: {:?}",
                addr,
                e
            );
        }
    }

    match id_pool.lock().unwrap().release_id(channel_id) {
        Ok(()) => {
            log::trace!("Released id {} for {:?}", channel_id, addr);
        }
        Err(e) => {
            log::error!("Error releasing id {} for {:?}: {:?}", channel_id, addr, e);
        }
    }

    match connection_counter_tx
        .send(ConnectionEvent::Disconnected)
        .await
    {
        Ok(()) => {
            log::trace!("Connection count updated for {:?}", addr);
        }
        Err(e) => {
            log::error!("Error updating connection count for {:?}: {:?}", addr, e);
        }
    }
}

async fn connection_loop(
    mut socket: WebSocket,
    addr: SocketAddr,
    mpv: Mpv,
    channel_id: u64,
    mut id_count_watch_receiver: watch::Receiver<u64>,
) -> Result<(), anyhow::Error> {
    let mut event_stream = mpv.get_event_stream().await;
    loop {
        select! {
          id_count = id_count_watch_receiver.changed() => {
            if let Err(e) = id_count {
              anyhow::bail!("Error reading id count watch receiver for {:?}: {:?}", addr, e);
            }

            let message = Message::Text(json!({
              "type": "connection_count",
              "value": id_count_watch_receiver.borrow().clone(),
            }).to_string());

            socket.send(message).await?;
          }
          message = socket.recv() => {
              log::trace!("Received command from {:?}: {:?}", addr, message);

              let ws_message_content = message
              .ok_or(anyhow::anyhow!("Event stream ended for {:?}", addr))
              .and_then(|message| {
                match message {
                  Ok(message) => Ok(message),
                  err => Err(anyhow::anyhow!("Error reading message for {:?}: {:?}", addr, err)),
                }
              })?;

              if let Message::Close(_) = ws_message_content {
                log::trace!("Closing connection for {:?}", addr);
                return Ok(());
              }

              if let Message::Ping(xs) = ws_message_content {
                log::trace!("Ponging {:?} with {:?}", addr, xs);
                socket.send(Message::Pong(xs)).await?;
                continue;
              }

              let message_content = match ws_message_content {
                  Message::Text(text) => text,
                  m => anyhow::bail!("Unexpected message type: {:?}", m),
              };

              let message_json = match serde_json::from_str::<Value>(&message_content) {
                  Ok(json) => json,
                  Err(e) => anyhow::bail!("Error parsing message from {:?}: {:?}", addr, e),
              };

              log::trace!("Handling command from {:?}: {:?}", addr, message_json);

              // TODO: handle errors
              match handle_message(message_json, mpv.clone(), channel_id).await {
                Ok(Some(response)) => {
                  log::trace!("Handled command from {:?} successfully, sending response", addr);
                  let message = Message::Text(json!({
                    "type": "response",
                    "value": response,
                  }).to_string());
                  socket.send(message).await?;
                }
                Ok(None) => {
                  log::trace!("Handled command from {:?} successfully", addr);
                }
                Err(e) => {
                  log::error!("Error handling message from {:?}: {:?}", addr, e);
                }
              }
          }
          event = event_stream.next() => {
            match event {
              Some(Ok(event)) => {
                log::trace!("Sending event to {:?}: {:?}", addr, event);
                let message = Message::Text(json!({
                  "type": "event",
                  "value": event,
                }).to_string());
                socket.send(message).await?;
              }
              Some(Err(e)) => {
                log::error!("Error reading event stream for {:?}: {:?}", addr, e);
                anyhow::bail!("Error reading event stream for {:?}: {:?}", addr, e);
              }
              None => {
                log::trace!("Event stream ended for {:?}", addr);
                return Ok(());
              }
            }
          }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WSCommand {
    // Subscribe { property: String },
    // UnsubscribeAll,
    Load { urls: Vec<String> },
    TogglePlayback,
    Volume { volume: f64 },
    Time { time: f64 },
    PlaylistNext,
    PlaylistPrevious,
    PlaylistGoto { position: usize },
    PlaylistClear,
    PlaylistRemove { positions: Vec<usize> },
    PlaylistMove { from: usize, to: usize },
    Shuffle,
    SetSubtitleTrack { track: Option<usize> },
    SetLooping { value: bool },
}

async fn handle_message(
    message: Value,
    mpv: Mpv,
    _channel_id: u64,
) -> anyhow::Result<Option<Value>> {
    let command =
        serde_json::from_value::<WSCommand>(message).context("Failed to parse message")?;

    log::trace!("Successfully parsed message: {:?}", command);

    match command {
        // WSCommand::Subscribe { property } => {
        //     mpv.observe_property(channel_id, &property).await?;
        //     Ok(None)
        // }
        // WSCommand::UnsubscribeAll => {
        //     mpv.unobserve_property(channel_id).await?;
        //     Ok(None)
        // }
        WSCommand::Load { urls } => {
            for url in urls {
                mpv.playlist_add(
                    &url,
                    PlaylistAddTypeOptions::File,
                    mpvipc_async::PlaylistAddOptions::Append,
                )
                .await?;
            }
            Ok(None)
        }
        WSCommand::TogglePlayback => {
            mpv.set_playback(mpvipc_async::Switch::Toggle).await?;
            Ok(None)
        }
        WSCommand::Volume { volume } => {
            mpv.set_volume(volume, NumberChangeOptions::Absolute)
                .await?;
            Ok(None)
        }
        WSCommand::Time { time } => {
            mpv.seek(time, SeekOptions::AbsolutePercent).await?;
            Ok(None)
        }
        WSCommand::PlaylistNext => {
            mpv.next().await?;
            Ok(None)
        }
        WSCommand::PlaylistPrevious => {
            mpv.prev().await?;
            Ok(None)
        }
        WSCommand::PlaylistGoto { position } => {
            mpv.playlist_play_id(position).await?;
            Ok(None)
        }
        WSCommand::PlaylistClear => {
            mpv.playlist_clear().await?;
            Ok(None)
        }

        // FIXME: this could lead to a race condition between `playlist_remove_id` commands
        WSCommand::PlaylistRemove { mut positions } => {
            positions.sort();

            for position in positions.iter().rev() {
                mpv.playlist_remove_id(*position).await?;
            }

            Ok(None)
        }

        WSCommand::PlaylistMove { from, to } => {
            mpv.playlist_move_id(from, to).await?;
            Ok(None)
        }
        WSCommand::Shuffle => {
            mpv.playlist_shuffle().await?;
            Ok(None)
        }
        WSCommand::SetSubtitleTrack { track } => {
            mpv.set_property("sid", track).await?;
            Ok(None)
        }
        WSCommand::SetLooping { value } => {
            mpv.set_loop_playlist(if value { Switch::On } else { Switch::Off })
                .await?;
            Ok(None)
        }
    }
}
