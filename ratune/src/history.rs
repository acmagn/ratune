use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ── Data structures ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayRecord {
    pub song_id: String,
    pub album_id: String,
    pub artist_id: String,
    pub artist_name: String,
    pub album_name: String,
    pub track_name: String,
    /// Unix timestamp in seconds.
    pub played_at: i64,
    pub duration_secs: u64,
}

impl PlayRecord {
    /// Convenience: current Unix timestamp (seconds).
    pub fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PlayHistory {
    pub records: Vec<PlayRecord>,
}

// ── Methods ───────────────────────────────────────────────────────────────────

const MAX_RECORDS: usize = 10_000;

impl PlayHistory {
    /// Append a play record, capping the history at 10,000 entries (drops oldest).
    pub fn record_play(&mut self, record: PlayRecord) {
        self.records.push(record);
        if self.records.len() > MAX_RECORDS {
            let excess = self.records.len() - MAX_RECORDS;
            self.records.drain(0..excess);
        }
    }

    /// Returns `(album_id, album_name, artist_name)` for the N most recently
    /// played unique albums, deduplicated by `album_id`, ordered by most recent
    /// `played_at` descending.
    pub fn recent_albums(&self, n: usize) -> Vec<(String, String, String)> {
        let mut seen: Vec<String> = Vec::new();
        let mut result: Vec<(String, String, String)> = Vec::new();

        for record in self.records.iter().rev() {
            if !seen.contains(&record.album_id) {
                seen.push(record.album_id.clone());
                result.push((
                    record.album_id.clone(),
                    record.album_name.clone(),
                    record.artist_name.clone(),
                ));
                if result.len() >= n {
                    break;
                }
            }
        }
        result
    }

    /// Returns `(artist_id, artist_name, play_count)` sorted by play count descending.
    pub fn top_artists(&self, n: usize) -> Vec<(String, String, u64)> {
        // Map artist_id -> (artist_name, count)
        let mut map: HashMap<String, (String, u64)> = HashMap::new();
        for record in &self.records {
            let entry = map
                .entry(record.artist_id.clone())
                .or_insert_with(|| (record.artist_name.clone(), 0));
            entry.1 += 1;
        }

        let mut artists: Vec<(String, String, u64)> = map
            .into_iter()
            .map(|(id, (name, count))| (id, name, count))
            .collect();

        artists.sort_by_key(|a| std::cmp::Reverse(a.2));
        artists.truncate(n);
        artists
    }

    /// Returns artists from `library_artist_ids` that have not been played
    /// recently, biased toward low play counts.
    ///
    /// Thresholds tried in order: 14 days → 7 days → 3 days → random from library.
    /// Returns at most `n` `(artist_id, artist_name)` pairs.
    pub fn rediscover_artists(
        &self,
        n: usize,
        library_artist_ids: &[(String, String)],
    ) -> Vec<(String, String)> {
        if n == 0 || library_artist_ids.is_empty() {
            return Vec::new();
        }

        let now = PlayRecord::now_secs();

        // Build: artist_id -> (last_played_at, play_count)
        let mut stats: HashMap<String, (i64, u64)> = HashMap::new();
        for record in &self.records {
            let entry = stats
                .entry(record.artist_id.clone())
                .or_insert((record.played_at, 0));
            if record.played_at > entry.0 {
                entry.0 = record.played_at;
            }
            entry.1 += 1;
        }

        let secs_per_day: i64 = 86_400;

        for threshold_days in [14i64, 7, 3] {
            let cutoff = now - threshold_days * secs_per_day;
            let mut candidates: Vec<(String, String, u64)> = library_artist_ids
                .iter()
                .filter(|(id, _)| {
                    stats
                        .get(id)
                        .map(|(last, _)| *last < cutoff)
                        .unwrap_or(true) // never played → always qualifies
                })
                .map(|(id, name)| {
                    let count = stats.get(id).map(|(_, c)| *c).unwrap_or(0);
                    (id.clone(), name.clone(), count)
                })
                .collect();

            if candidates.len() >= n {
                // Shuffle within play-count tiers so results vary on each re-roll.
                use rand::seq::SliceRandom;
                let mut rng = rand::thread_rng();
                candidates.shuffle(&mut rng);
                // Then stable-sort by play_count so least-played still come first,
                // but ties are in random order.
                candidates.sort_by_key(|(_, _, c)| *c);
                return candidates
                    .into_iter()
                    .take(n)
                    .map(|(id, name, _)| (id, name))
                    .collect();
            }
        }

        // Fallback: shuffle the library slice so each re-roll picks differently.
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();
        let mut all: Vec<(String, String)> = library_artist_ids
            .iter()
            .map(|(id, name)| (id.clone(), name.clone()))
            .collect();
        all.shuffle(&mut rng);
        all.into_iter().take(n).collect()
    }

    /// Load from a JSON file.  Returns `Default` if the file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading history file {}", path.display()))?;
        let history: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing history file {}", path.display()))?;
        Ok(history)
    }

    /// Serialize to JSON, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating history dir {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
            .with_context(|| format!("writing history to {}", path.display()))?;
        Ok(())
    }
}

// ── Path helper ───────────────────────────────────────────────────────────────

/// Returns `~/.local/share/ratune/history.json`.
/// Computed from `$HOME` (no `dirs` crate needed — it is not yet in Cargo.toml).
pub fn history_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("ratune")
            .join("history.json")
    } else {
        // Unlikely fallback: relative path.
        std::path::PathBuf::from("history.json")
    }
}
