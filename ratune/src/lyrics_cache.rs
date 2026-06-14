//! On-disk lyrics cache under `~/.cache/ratune/lyrics/`.
//!
//! Lyrics are stored as small JSON files keyed by song ID so they remain
//! available offline alongside cached audio tracks.

use std::path::{Path, PathBuf};

use ratune_subsonic::LyricLine;
use serde::{Deserialize, Serialize};

use crate::cache::ratune_cache_dir;

#[derive(Serialize, Deserialize)]
struct CachedLine {
    time_ms: Option<u64>,
    text: String,
}

#[derive(Serialize, Deserialize)]
struct CachedLyrics {
    lines: Vec<CachedLine>,
}

/// Disk-backed lyrics store.
#[derive(Clone)]
pub struct LyricsDiskCache {
    dir: PathBuf,
}

impl LyricsDiskCache {
    /// Open the lyrics cache directory, creating it when possible.
    pub fn load() -> Self {
        let dir = ratune_cache_dir()
            .map(|base| base.join("lyrics"))
            .unwrap_or_else(|| PathBuf::from(".ratune-lyrics-cache"));
        let _ = std::fs::create_dir_all(&dir);
        Self { dir }
    }

    fn path_for(&self, source: &str, song_id: &str) -> PathBuf {
        self.dir.join(source).join(format!("{song_id}.json"))
    }

    /// Directory where lyrics JSON files are stored for a given source.
    pub fn cache_dir_for(&self, source: &str) -> PathBuf {
        let dir = self.dir.join(source);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Read cached lyrics for `song_id`, if present.
    pub fn get(&self, source: &str, song_id: &str) -> Option<Vec<LyricLine>> {
        let path = self.path_for(source, song_id);
        let json = std::fs::read_to_string(path).ok()?;
        let cached: CachedLyrics = serde_json::from_str(&json).ok()?;
        Some(cached.lines.into_iter().map(line_from_cached).collect())
    }

    /// Write lyrics to a source-specific directory (for async fetch tasks).
    pub fn put_at(dir: &Path, song_id: &str, lines: &[LyricLine]) {
        let cached = CachedLyrics {
            lines: lines.iter().map(line_to_cached).collect(),
        };
        if let Ok(json) = serde_json::to_string(&cached) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join(format!("{song_id}.json")), json);
        }
    }
}

fn line_to_cached(line: &LyricLine) -> CachedLine {
    CachedLine {
        time_ms: line.time.map(|d| d.as_millis() as u64),
        text: line.text.clone(),
    }
}

fn line_from_cached(line: CachedLine) -> LyricLine {
    LyricLine {
        time: line.time_ms.map(std::time::Duration::from_millis),
        text: line.text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_cached_lines() {
        let lines = vec![
            LyricLine {
                time: Some(std::time::Duration::from_millis(1500)),
                text: "Hello".into(),
            },
            LyricLine {
                time: None,
                text: "World".into(),
            },
        ];
        let dir = std::env::temp_dir().join(format!("ratune-lyrics-test-{}", std::process::id()));
        let source_dir = dir.join("lrclib");
        let _ = std::fs::create_dir_all(&source_dir);
        LyricsDiskCache::put_at(&source_dir, "song-1", &lines);
        let cache = LyricsDiskCache { dir };
        let loaded = cache.get("lrclib", "song-1").expect("cached lyrics");
        assert_eq!(loaded, lines);
        let _ = std::fs::remove_dir_all(cache.dir);
    }

    #[test]
    fn cache_isolated_by_source() {
        let dir =
            std::env::temp_dir().join(format!("ratune-lyrics-sources-{}", std::process::id()));
        let lrclib_dir = dir.join("lrclib");
        let subsonic_dir = dir.join("subsonic");
        let lrclib_lines = vec![LyricLine {
            time: None,
            text: "from lrclib".into(),
        }];
        let subsonic_lines = vec![LyricLine {
            time: None,
            text: "from subsonic".into(),
        }];
        LyricsDiskCache::put_at(&lrclib_dir, "song-1", &lrclib_lines);
        LyricsDiskCache::put_at(&subsonic_dir, "song-1", &subsonic_lines);
        let cache = LyricsDiskCache { dir };
        assert_eq!(
            cache.get("lrclib", "song-1").expect("lrclib")[0].text,
            "from lrclib"
        );
        assert_eq!(
            cache.get("subsonic", "song-1").expect("subsonic")[0].text,
            "from subsonic"
        );
        let _ = std::fs::remove_dir_all(cache.dir);
    }

    #[test]
    fn empty_lines_are_cached() {
        let dir = std::env::temp_dir().join(format!("ratune-lyrics-empty-{}", std::process::id()));
        let source_dir = dir.join("subsonic");
        LyricsDiskCache::put_at(&source_dir, "no-lyrics", &[]);
        let cache = LyricsDiskCache { dir };
        let loaded = cache.get("subsonic", "no-lyrics").expect("cached empty");
        assert!(loaded.is_empty());
        let _ = std::fs::remove_dir_all(cache.dir);
    }

    #[test]
    fn missing_cache_entry_returns_none() {
        let cache = LyricsDiskCache {
            dir: std::env::temp_dir().join(format!("ratune-lyrics-missing-{}", std::process::id())),
        };
        assert!(cache.get("lrclib", "does-not-exist").is_none());
    }
}
