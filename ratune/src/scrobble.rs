use std::sync::Arc;

use ratune_scrobble::{AudioscrobblerClient, TrackInfo};
use ratune_subsonic::{Song, SubsonicClient};
use tokio::sync::mpsc;

use crate::app::LibraryUpdate;
use crate::scrobble_queue::QueuedScrobble;

pub fn track_from_song(song: &Song) -> TrackInfo {
    TrackInfo {
        song_id: song.id.clone(),
        artist: song
            .artist
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown Artist".into()),
        title: song.title.clone(),
        album: song.album.clone().filter(|s| !s.is_empty()),
        track_number: song.track,
        duration_secs: song.duration,
    }
}

pub fn spawn_now_playing(client: AudioscrobblerClient, track: TrackInfo) {
    tokio::spawn(async move {
        if let Err(e) = client.update_now_playing(&track).await {
            eprintln!("scrobble: now playing failed: {e:#}");
        }
    });
}

pub fn spawn_audioscrobbler_scrobble(
    client: AudioscrobblerClient,
    track: TrackInfo,
    timestamp: i64,
    tx: mpsc::Sender<LibraryUpdate>,
    from_live: bool,
) {
    tokio::spawn(async move {
        let artist = track.artist.clone();
        let title = track.title.clone();
        let result = client
            .scrobble(&track, timestamp)
            .await
            .map_err(|e| e.to_string());
        let _ = tx
            .send(LibraryUpdate::ScrobbleResult {
                entry: QueuedScrobble::from_track(&track, timestamp),
                result,
                artist,
                title,
                from_live,
            })
            .await;
    });
}

pub fn spawn_subsonic_scrobble(client: Arc<SubsonicClient>, song_id: String) {
    tokio::spawn(async move {
        if let Err(e) = client.scrobble(&song_id).await {
            eprintln!("scrobble: server scrobble failed: {e:#}");
        }
    });
}

pub fn spawn_flush_scrobble_queue(
    client: AudioscrobblerClient,
    entries: Vec<QueuedScrobble>,
    tx: mpsc::Sender<LibraryUpdate>,
) {
    if entries.is_empty() {
        return;
    }
    tokio::spawn(async move {
        for entry in entries {
            let artist = entry.artist.clone();
            let title = entry.title.clone();
            let timestamp = entry.timestamp;
            let track = entry.to_track_info();
            let result = client
                .scrobble(&track, timestamp)
                .await
                .map_err(|e| e.to_string());
            let _ = tx
                .send(LibraryUpdate::ScrobbleResult {
                    entry,
                    result,
                    artist,
                    title,
                    from_live: false,
                })
                .await;
        }
    });
}
