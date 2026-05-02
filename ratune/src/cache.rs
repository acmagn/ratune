//! Offline track cache.
//!
//! Stores downloaded audio files in `~/.cache/ratune/tracks/` and maintains
//! a JSON index at `~/.cache/ratune/cache.json`. LRU eviction removes the
//! least-recently-played entries when the configured size limit is exceeded.
//!
//! Cache hits are read directly by the audio engine from disk.
//! Writes use a temp file + rename so the index never points at a half-written file.
//!
//! All IO errors are soft-failed — the cache never crashes or interrupts playback.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Types ─────────────────────────────────────────────────────────────────────

/// One entry in the cache index.
#[derive(Serialize, Deserialize, Clone)]
pub struct CacheEntry {
    /// Absolute path to the cached file on disk.
    pub path: PathBuf,
    /// Size of the cached file in bytes.
    pub size_bytes: u64,
    /// Unix timestamp (seconds) when this track was last played. Used for LRU.
    pub last_played: u64,
    /// Subsonic album ID — stored for potential future album-level eviction.
    pub album_id: String,
}

/// In-memory representation of the track cache.
pub struct TrackCache {
    /// Index of cached songs by song ID.
    entries: HashMap<String, CacheEntry>,
    /// `~/.cache/ratune/tracks/`
    tracks_dir: PathBuf,
    /// `~/.cache/ratune/cache.json`
    index_path: PathBuf,
    /// Eviction threshold in bytes.
    max_size_bytes: u64,
    /// Set to false when the cache dir is unwritable or the feature is disabled.
    pub enabled: bool,
}

// ── Directory helpers ─────────────────────────────────────────────────────────

fn cache_base_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg).join("ratune"));
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".cache").join("ratune"))
}

/// `~/.cache/ratune` (or `$XDG_CACHE_HOME/ratune`).
pub fn ratune_cache_dir() -> Option<PathBuf> {
    cache_base_dir()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Implementation ────────────────────────────────────────────────────────────

impl TrackCache {
    /// Load the cache from disk. Silently starts with an empty index if the
    /// index file is missing or malformed. Disables itself if the cache dir
    /// cannot be created.
    pub fn load(enabled: bool, max_size_gb: f64) -> Self {
        let max_size_bytes = (max_size_gb * 1024.0 * 1024.0 * 1024.0) as u64;

        let Some(base) = cache_base_dir() else {
            eprintln!("warn: could not determine cache directory — caching disabled");
            return Self::disabled(max_size_bytes);
        };

        let tracks_dir = base.join("tracks");
        let index_path = base.join("cache.json");

        if !enabled {
            return Self {
                entries: HashMap::new(),
                tracks_dir,
                index_path,
                max_size_bytes,
                enabled: false,
            };
        }

        if let Err(e) = std::fs::create_dir_all(&tracks_dir) {
            eprintln!(
                "warn: could not create cache dir {}: {e} — caching disabled",
                tracks_dir.display()
            );
            return Self {
                entries: HashMap::new(),
                tracks_dir,
                index_path,
                max_size_bytes,
                enabled: false,
            };
        }

        let entries: HashMap<String, CacheEntry> = if index_path.exists() {
            std::fs::read_to_string(&index_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        Self {
            entries,
            tracks_dir,
            index_path,
            max_size_bytes,
            enabled: true,
        }
    }

    fn disabled(max_size_bytes: u64) -> Self {
        Self {
            entries: HashMap::new(),
            tracks_dir: PathBuf::new(),
            index_path: PathBuf::new(),
            max_size_bytes,
            enabled: false,
        }
    }

    /// Return `true` if `song_id` appears in the index with an existing file.
    ///
    /// Non-mutating — does not remove stale entries. Use this when only a
    /// presence check is needed and you do not hold a `&mut` already.
    pub fn get_const(&self, song_id: &str) -> bool {
        if !self.enabled {
            return false;
        }
        self.entries
            .get(song_id)
            .map(|e| e.path.exists())
            .unwrap_or(false)
    }

    /// Return the cached file path for `song_id` if the file exists on disk.
    ///
    /// Removes stale index entries whose files have been deleted externally.
    pub fn get(&mut self, song_id: &str) -> Option<PathBuf> {
        if !self.enabled {
            return None;
        }
        if let Some(entry) = self.entries.get(song_id) {
            if entry.path.exists() {
                return Some(entry.path.clone());
            }
            // File disappeared — remove stale entry without saving (save on next write).
            self.entries.remove(song_id);
        }
        None
    }

    /// Write `data` to the cache for `song_id`.
    ///
    /// Updates the index, runs LRU eviction, then saves. Any IO error is
    /// logged but does not propagate — cache failures must never affect playback.
    pub fn put(&mut self, song_id: &str, album_id: &str, data: &[u8]) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let path = self.tracks_dir.join(format!("{song_id}.cache"));
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = self
            .tracks_dir
            .join(format!("{song_id}.cache.{nanos}.part"));
        if let Err(e) = std::fs::write(&temp, data) {
            eprintln!("warn: cache write failed for {song_id}: {e}");
            return Ok(());
        }
        // Replace existing file atomically where the platform allows it, so readers
        // never see a truncated file while a download completes.
        let _ = std::fs::remove_file(&path);
        if let Err(e) = std::fs::rename(&temp, &path) {
            eprintln!("warn: cache finalize failed for {song_id}: {e}");
            let _ = std::fs::remove_file(&temp);
            return Ok(());
        }

        self.entries.insert(
            song_id.to_string(),
            CacheEntry {
                path,
                size_bytes: data.len() as u64,
                last_played: unix_now(),
                album_id: album_id.to_string(),
            },
        );

        self.evict_to_limit();
        self.save_index();
        Ok(())
    }

    /// Update the `last_played` timestamp for `song_id` (LRU clock tick).
    pub fn touch(&mut self, song_id: &str) {
        if !self.enabled {
            return;
        }
        if let Some(entry) = self.entries.get_mut(song_id) {
            entry.last_played = unix_now();
            self.save_index();
        }
    }

    /// Evict the least-recently-played entries until the total cache size is
    /// under `max_size_bytes`. Also saves the index after eviction.
    pub fn evict_to_limit(&mut self) {
        if !self.enabled {
            return;
        }

        let total: u64 = self.entries.values().map(|e| e.size_bytes).sum();
        if total <= self.max_size_bytes {
            return;
        }

        // Sort by last_played ascending — oldest entries evicted first.
        let mut by_age: Vec<(String, u64)> = self
            .entries
            .iter()
            .map(|(id, e)| (id.clone(), e.last_played))
            .collect();
        by_age.sort_by_key(|(_, t)| *t);

        let mut remaining = total;
        for (id, _) in by_age {
            if remaining <= self.max_size_bytes {
                break;
            }
            if let Some(entry) = self.entries.remove(&id) {
                if let Err(e) = std::fs::remove_file(&entry.path) {
                    eprintln!(
                        "warn: cache eviction failed for {}: {e}",
                        entry.path.display()
                    );
                }
                remaining = remaining.saturating_sub(entry.size_bytes);
            }
        }
        self.save_index();
    }

    /// Serialize the index to `cache.json`. Silently ignores all errors.
    fn save_index(&self) {
        if !self.enabled {
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.entries) {
            let _ = std::fs::write(&self.index_path, json);
        }
    }
}
