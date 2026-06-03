use std::time::{SystemTime, UNIX_EPOCH};

/// Metadata sent to Audioscrobbler-compatible services.
#[derive(Debug, Clone)]
pub struct TrackInfo {
    /// Subsonic song id — used for server-side scrobbling, not sent to Last.fm.
    pub song_id: String,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub track_number: Option<u32>,
    /// Track length in whole seconds.
    pub duration_secs: Option<u32>,
}

impl TrackInfo {
    pub fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }
}
