use std::sync::Arc;

use ratune_scrobble::{AudioscrobblerClient, TrackInfo};
use ratune_subsonic::{Song, SubsonicClient};

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
) {
    tokio::spawn(async move {
        if let Err(e) = client.scrobble(&track, timestamp).await {
            eprintln!("scrobble: submit failed: {e:#}");
        }
    });
}

pub fn spawn_subsonic_scrobble(client: Arc<SubsonicClient>, song_id: String) {
    tokio::spawn(async move {
        if let Err(e) = client.scrobble(&song_id).await {
            eprintln!("scrobble: server scrobble failed: {e:#}");
        }
    });
}
