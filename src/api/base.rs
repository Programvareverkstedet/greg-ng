use mpvipc_async::{
    LoopProperty, Mpv, MpvExt, NumberChangeOptions, PlaylistAddOptions, PlaylistAddTypeOptions,
    SeekOptions, Switch,
};
use serde_json::{json, Value};

/// Add item to playlist
pub async fn loadfile(mpv: Mpv, path: &str) -> anyhow::Result<()> {
    log::trace!("api::loadfile({:?})", path);
    mpv.playlist_add(
        path,
        PlaylistAddTypeOptions::File,
        PlaylistAddOptions::Append,
    )
    .await?;

    Ok(())
}

/// Check whether the player is paused or playing
pub async fn play_get(mpv: Mpv) -> anyhow::Result<Value> {
    log::trace!("api::play_get()");
    let paused: bool = !mpv.is_playing().await?;
    Ok(json!(!paused))
}

/// Set whether the player is paused or playing
pub async fn play_set(mpv: Mpv, should_play: bool) -> anyhow::Result<()> {
    log::trace!("api::play_set({:?})", should_play);
    mpv.set_playback(if should_play { Switch::On } else { Switch::Off })
        .await
        .map_err(|e| e.into())
}

/// Get the current player volume
pub async fn volume_get(mpv: Mpv) -> anyhow::Result<Value> {
    log::trace!("api::volume_get()");
    let volume: f64 = mpv.get_volume().await?;
    Ok(json!(volume))
}

/// Set the player volume
pub async fn volume_set(mpv: Mpv, value: f64) -> anyhow::Result<()> {
    log::trace!("api::volume_set({:?})", value);
    mpv.set_volume(value, NumberChangeOptions::Absolute)
        .await
        .map_err(|e| e.into())
}

/// Get current playback position
pub async fn time_get(mpv: Mpv) -> anyhow::Result<Value> {
    log::trace!("api::time_get()");
    let current: Option<f64> = mpv.get_time_pos().await?;
    let remaining: Option<f64> = mpv.get_time_remaining().await?;
    let total = match (current, remaining) {
        (Some(c), Some(r)) => Some(c + r),
        (_, _) => None,
    };

    Ok(json!({
        "current": current,
        "remaining": remaining,
        "total": total,
    }))
}

/// Set playback position
pub async fn time_set(mpv: Mpv, pos: Option<f64>, percent: Option<f64>) -> anyhow::Result<()> {
    log::trace!("api::time_set({:?}, {:?})", pos, percent);
    if pos.is_some() && percent.is_some() {
        anyhow::bail!("pos and percent cannot be provided at the same time");
    }

    if let Some(pos) = pos {
        mpv.seek(pos, SeekOptions::Absolute).await?;
    } else if let Some(percent) = percent {
        mpv.seek(percent, SeekOptions::AbsolutePercent).await?;
    } else {
        anyhow::bail!("Either pos or percent must be provided");
    };

    Ok(())
}

/// Get the current playlist
pub async fn playlist_get(mpv: Mpv) -> anyhow::Result<Value> {
    log::trace!("api::playlist_get()");
    let playlist: mpvipc_async::Playlist = mpv.get_playlist().await?;
    let is_playing: bool = mpv.is_playing().await?;

    let items: Vec<Value> = playlist
        .0
        .iter()
        .enumerate()
        .map(|(i, item)| {
            json!({
              "index": i,
              "current": item.current,
              "playing": is_playing,
              "filename": item.title.as_ref().unwrap_or(&item.filename),
              "data": {
                "fetching": true,
              }
            })
        })
        .collect();

    Ok(json!(items))
}

/// Skip to the next item in the playlist
pub async fn playlist_next(mpv: Mpv) -> anyhow::Result<()> {
    log::trace!("api::playlist_next()");
    mpv.next().await.map_err(|e| e.into())
}

/// Go back to the previous item in the playlist
pub async fn playlist_previous(mpv: Mpv) -> anyhow::Result<()> {
    log::trace!("api::playlist_previous()");
    mpv.prev().await.map_err(|e| e.into())
}

/// Go chosen item in the playlist
pub async fn playlist_goto(mpv: Mpv, index: usize) -> anyhow::Result<()> {
    log::trace!("api::playlist_goto({:?})", index);
    mpv.playlist_play_id(index).await.map_err(|e| e.into())
}

/// Clears the playlist
pub async fn playlist_clear(mpv: Mpv) -> anyhow::Result<()> {
    log::trace!("api::playlist_clear()");
    mpv.playlist_clear().await.map_err(|e| e.into())
}

/// Remove an item from the playlist by index
pub async fn playlist_remove(mpv: Mpv, index: usize) -> anyhow::Result<()> {
    log::trace!("api::playlist_remove({:?})", index);
    mpv.playlist_remove_id(index).await.map_err(|e| e.into())
}

/// Move an item in the playlist from one index to another
pub async fn playlist_move(mpv: Mpv, from: usize, to: usize) -> anyhow::Result<()> {
    log::trace!("api::playlist_move({:?}, {:?})", from, to);
    mpv.playlist_move_id(from, to).await.map_err(|e| e.into())
}

/// Shuffle the playlist
pub async fn shuffle(mpv: Mpv) -> anyhow::Result<()> {
    log::trace!("api::shuffle()");
    mpv.playlist_shuffle().await.map_err(|e| e.into())
}

/// See whether it loops the playlist or not
pub async fn playlist_get_looping(mpv: Mpv) -> anyhow::Result<Value> {
    log::trace!("api::playlist_get_looping()");

    let loop_status = match mpv.playlist_is_looping().await? {
        LoopProperty::No => false,
        LoopProperty::Inf => true,
        LoopProperty::N(_) => true,
    };

    Ok(json!(loop_status))
}

pub async fn playlist_set_looping(mpv: Mpv, r#loop: bool) -> anyhow::Result<()> {
    log::trace!("api::playlist_set_looping({:?})", r#loop);

    mpv.set_loop_playlist(if r#loop { Switch::On } else { Switch::Off })
        .await
        .map_err(|e| e.into())
}

use swayipc::{Connection, Fallible};

pub async fn sway_run_command(command: String) -> Fallible<()> {
    tokio::task::spawn_blocking(move || -> Fallible<()> {
        let mut connection = Connection::new()?;
        connection.run_command(&command)?;
        Ok(())
    })
    .await
    .map_err(|e| swayipc::Error::CommandFailed(e.to_string()))?
}


//only to check if workspace exists.
fn get_workspace_names(connection: &mut Connection) -> Fallible<Vec<String>> {
    let workspaces = connection.get_workspaces()?;
    Ok(workspaces.iter().map(|w| w.name.clone()).collect())
}

pub async fn sway_get_workspace_names() -> Fallible<Vec<String>> {
    tokio::task::spawn_blocking(|| -> Fallible<Vec<String>> {
        let mut connection = Connection::new()?;
        get_workspace_names(&mut connection)
    })
    .await
    .map_err(|e| swayipc::Error::CommandFailed(e.to_string()))?
}

fn is_valid_workspace(workspace: &str, connection: &mut Connection) -> Fallible<bool> {
    let workspace_names = get_workspace_names(connection)?;
    Ok(workspace_names.contains(&workspace.to_string()) || 
        workspace.parse::<u32>()
            .map(|num| num >= 1 && num <= 10)
            .unwrap_or(false))
}

pub async fn sway_change_workspace(workspace: String) -> Fallible<()> {
    tokio::task::spawn_blocking(move || -> Fallible<()> {
        let mut connection = Connection::new()?;
        
        if !is_valid_workspace(&workspace, &mut connection)? {
            return Err(swayipc::Error::CommandFailed(
                "Invalid workspace name. Must be existing workspace or number 1-10".to_string()
            ));
        }

        connection.run_command(&format!("workspace {}", workspace))?;
        Ok(())
    })
    .await
    .map_err(|e| swayipc::Error::CommandFailed(e.to_string()))?
}

use url::Url;
pub async fn sway_launch_browser(url: &str) -> Fallible<()> {
    // Validate URL
    let url = Url::parse(url)
        .map_err(|e| swayipc::Error::CommandFailed(format!("Invalid URL: {}", e)))?;
    
    // Ensure URL scheme is http or https
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(swayipc::Error::CommandFailed("URL must use http or https protocol".into()));
    }

    tokio::task::spawn_blocking(move || -> Fallible<()> {
        let mut connection = Connection::new()?;
        connection.run_command(&format!("exec xdg-open {}", url))?;
        Ok(())
    })
    .await
    .map_err(|e| swayipc::Error::CommandFailed(e.to_string()))?
}

pub async fn sway_close_workspace(workspace: String) -> Fallible<()> {
    tokio::task::spawn_blocking(move || -> Fallible<()> {
        let mut connection = Connection::new()?;
        
        // Validate workspace exists
        if !is_valid_workspace(&workspace, &mut connection)? {
            return Err(swayipc::Error::CommandFailed(
                "Invalid workspace name".to_string()
            ));
        }

        // Get workspace tree and find all nodes in target workspace
        let tree = connection.get_tree()?;
        let workspace_nodes = tree
            .nodes
            .iter()
            .flat_map(|output| &output.nodes)  // Get workspaces
            .find(|ws| ws.name.as_ref().map_or(false, |name| name == &workspace));

        // Kill all nodes in workspace if found
        if let Some(ws) = workspace_nodes {
            for container in ws.nodes.iter() {
                // Close each container in the workspace
                connection.run_command(&format!("[con_id={}] kill", container.id))?;
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| swayipc::Error::CommandFailed(e.to_string()))?
}

