use std::sync::Arc;

use log::trace;
use mpvipc::{
    Mpv, NumberChangeOptions, PlaylistAddOptions, PlaylistAddTypeOptions, SeekOptions, Switch,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Add item to playlist
pub async fn loadfile(mpv: Arc<Mutex<Mpv>>, path: &str) -> anyhow::Result<()> {
    trace!("api::loadfile({:?})", path);
    mpv.lock().await.playlist_add(
        path,
        PlaylistAddTypeOptions::File,
        PlaylistAddOptions::Append,
    )?;

    Ok(())
}

/// Check whether the player is paused or playing
pub async fn play_get(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<Value> {
    trace!("api::play_get()");
    let paused: bool = mpv.lock().await.get_property("pause")?;
    Ok(json!(!paused))
}

/// Set whether the player is paused or playing
pub async fn play_set(mpv: Arc<Mutex<Mpv>>, should_play: bool) -> anyhow::Result<()> {
    trace!("api::play_set({:?})", should_play);
    mpv.lock()
        .await
        .set_property("pause", !should_play)
        .map_err(|e| e.into())
}

/// Get the current player volume
pub async fn volume_get(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<Value> {
    trace!("api::volume_get()");
    let volume: f64 = mpv.lock().await.get_property("volume")?;
    Ok(json!(volume))
}

/// Set the player volume
pub async fn volume_set(mpv: Arc<Mutex<Mpv>>, value: f64) -> anyhow::Result<()> {
    trace!("api::volume_set({:?})", value);
    mpv.lock()
        .await
        .set_volume(value, NumberChangeOptions::Absolute)
        .map_err(|e| e.into())
}

/// Get current playback position
pub async fn time_get(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<Value> {
    trace!("api::time_get()");
    let current: f64 = mpv.lock().await.get_property("time-pos")?;
    let remaining: f64 = mpv.lock().await.get_property("time-remaining")?;
    let total = current + remaining;

    Ok(json!({
        "current": current,
        "remaining": remaining,
        "total": total,
    }))
}

/// Set playback position
pub async fn time_set(
    mpv: Arc<Mutex<Mpv>>,
    pos: Option<f64>,
    percent: Option<f64>,
) -> anyhow::Result<()> {
    trace!("api::time_set({:?}, {:?})", pos, percent);
    if pos.is_some() && percent.is_some() {
        anyhow::bail!("pos and percent cannot be provided at the same time");
    }

    if let Some(pos) = pos {
        mpv.lock().await.seek(pos, SeekOptions::Absolute)?;
    } else if let Some(percent) = percent {
        mpv.lock()
            .await
            .seek(percent, SeekOptions::AbsolutePercent)?;
    } else {
        anyhow::bail!("Either pos or percent must be provided");
    };

    Ok(())
}

/// Get the current playlist
pub async fn playlist_get(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<Value> {
    trace!("api::playlist_get()");
    let playlist: mpvipc::Playlist = mpv.lock().await.get_playlist()?;
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

    Ok(json!(items))
}

/// Skip to the next item in the playlist
pub async fn playlist_next(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<()> {
    trace!("api::playlist_next()");
    mpv.lock().await.next().map_err(|e| e.into())
}

/// Go back to the previous item in the playlist
pub async fn playlist_previous(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<()> {
    trace!("api::playlist_previous()");
    mpv.lock().await.prev().map_err(|e| e.into())
}

/// Go chosen item in the playlist
pub async fn playlist_goto(mpv: Arc<Mutex<Mpv>>, index: usize) -> anyhow::Result<()> {
    trace!("api::playlist_goto({:?})", index);
    mpv.lock()
        .await
        .playlist_play_id(index)
        .map_err(|e| e.into())
}

/// Clears the playlist
pub async fn playlist_clear(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<()> {
    trace!("api::playlist_clear()");
    mpv.lock().await.playlist_clear().map_err(|e| e.into())
}

/// Remove an item from the playlist by index
pub async fn playlist_remove(mpv: Arc<Mutex<Mpv>>, index: usize) -> anyhow::Result<()> {
    trace!("api::playlist_remove({:?})", index);
    mpv.lock()
        .await
        .playlist_remove_id(index)
        .map_err(|e| e.into())
}

/// Move an item in the playlist from one index to another
pub async fn playlist_move(mpv: Arc<Mutex<Mpv>>, from: usize, to: usize) -> anyhow::Result<()> {
    trace!("api::playlist_move({:?}, {:?})", from, to);
    mpv.lock()
        .await
        .playlist_move_id(from, to)
        .map_err(|e| e.into())
}

/// Shuffle the playlist
pub async fn shuffle(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<()> {
    trace!("api::shuffle()");
    mpv.lock().await.playlist_shuffle().map_err(|e| e.into())
}

/// See whether it loops the playlist or not
pub async fn playlist_get_looping(mpv: Arc<Mutex<Mpv>>) -> anyhow::Result<Value> {
    trace!("api::playlist_get_looping()");
    let loop_playlist = mpv.lock().await.get_property_string("loop-playlist")? == "inf";
    Ok(json!(loop_playlist))
}

pub async fn playlist_set_looping(mpv: Arc<Mutex<Mpv>>, r#loop: bool) -> anyhow::Result<()> {
    trace!("api::playlist_set_looping({:?})", r#loop);
    if r#loop {
        mpv.lock()
            .await
            .set_loop_playlist(Switch::On)
            .map_err(|e| e.into())
    } else {
        mpv.lock()
            .await
            .set_loop_playlist(Switch::Off)
            .map_err(|e| e.into())
    }
}
