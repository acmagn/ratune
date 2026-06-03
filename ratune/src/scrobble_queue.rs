use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use ratune_scrobble::TrackInfo;
use serde::{Deserialize, Serialize};

/// One scrobble waiting to be submitted after a network/API failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueuedScrobble {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub track_number: Option<u32>,
    pub duration_secs: Option<u32>,
    /// Unix seconds when playback started (Audioscrobbler timestamp).
    pub timestamp: i64,
}

impl QueuedScrobble {
    pub fn from_track(track: &TrackInfo, timestamp: i64) -> Self {
        Self {
            artist: track.artist.clone(),
            title: track.title.clone(),
            album: track.album.clone(),
            track_number: track.track_number,
            duration_secs: track.duration_secs,
            timestamp,
        }
    }

    pub fn to_track_info(&self) -> TrackInfo {
        TrackInfo {
            song_id: String::new(),
            artist: self.artist.clone(),
            title: self.title.clone(),
            album: self.album.clone(),
            track_number: self.track_number,
            duration_secs: self.duration_secs,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScrobbleQueue {
    pub entries: Vec<QueuedScrobble>,
}

const MAX_ENTRIES: usize = 500;
/// Last.fm rejects scrobbles older than ~14 days.
const MAX_AGE_SECS: i64 = 14 * 86_400;

impl ScrobbleQueue {
    pub fn push(&mut self, entry: QueuedScrobble) {
        if self.entries.iter().any(|e| e == &entry) {
            return;
        }
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(0..excess);
        }
    }

    pub fn drop_stale(&mut self) {
        let now = now_secs();
        self.entries
            .retain(|e| now.saturating_sub(e.timestamp) <= MAX_AGE_SECS);
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading scrobble queue {}", path.display()))?;
        let mut queue: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing scrobble queue {}", path.display()))?;
        queue.drop_stale();
        Ok(queue)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating scrobble queue dir {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
            .with_context(|| format!("writing scrobble queue {}", path.display()))?;
        Ok(())
    }
}

pub fn scrobble_queue_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("ratune")
            .join("scrobble-queue.json")
    } else {
        std::path::PathBuf::from("scrobble-queue.json")
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_entries_older_than_fourteen_days() {
        let mut q = ScrobbleQueue::default();
        q.entries.push(QueuedScrobble {
            artist: "A".into(),
            title: "T".into(),
            album: None,
            track_number: None,
            duration_secs: None,
            timestamp: now_secs() - MAX_AGE_SECS - 1,
        });
        q.drop_stale();
        assert!(q.is_empty());
    }
}
