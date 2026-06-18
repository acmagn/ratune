//! On-disk snapshot of the starred library (`getStarred2`) for offline favorites browsing.
//!
//! Written after each successful server sync; read on startup and when opening the
//! favorites overlay while offline.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ratune_subsonic::Starred2;
use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct FavoritesCacheFile {
    version: u32,
    refreshed_at_unix: u64,
    #[serde(flatten)]
    starred: Starred2,
}

/// Default path: `~/.cache/ratune/favorites.json`.
pub fn default_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("ratune").join("favorites.json");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
        .join(".cache")
        .join("ratune")
        .join("favorites.json")
}

/// Load a snapshot. Returns `(starred, refreshed_at_unix)`.
pub fn load(path: &Path) -> Option<(Starred2, u64)> {
    let text = fs::read_to_string(path).ok()?;
    let file: FavoritesCacheFile = serde_json::from_str(&text).ok()?;
    if file.version != FORMAT_VERSION {
        return None;
    }
    Some((file.starred.normalize(), file.refreshed_at_unix))
}

/// Atomically write the snapshot (temp + rename). Returns refresh timestamp (unix secs).
pub fn save(path: &Path, starred: &Starred2) -> Result<u64> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let refreshed_at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file = FavoritesCacheFile {
        version: FORMAT_VERSION,
        refreshed_at_unix,
        starred: starred.clone(),
    };
    let json = serde_json::to_string_pretty(&file).context("serialize favorites cache")?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!("json.{nanos}.part"));
    let mut f = fs::File::create(&temp).with_context(|| format!("writing {}", temp.display()))?;
    f.write_all(json.as_bytes())?;
    f.sync_all().ok();
    drop(f);
    fs::rename(&temp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(refreshed_at_unix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratune_subsonic::Song;

    #[test]
    fn roundtrip_save_load() {
        let dir = std::env::temp_dir().join(format!("ratune-fav-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("favorites.json");

        let mut starred = Starred2::default();
        starred.song.push(Song {
            id: "1".into(),
            title: "Loved".into(),
            album: Some("Alb".into()),
            artist: Some("Art".into()),
            album_id: Some("al1".into()),
            artist_id: None,
            track: None,
            disc_number: None,
            year: None,
            genre: None,
            cover_art: None,
            duration: Some(200),
            bit_rate: None,
            content_type: None,
            suffix: None,
            size: None,
            path: None,
            starred: Some("2024-01-01T00:00:00Z".into()),
        });

        let ts = save(&path, &starred).unwrap();
        let (loaded, loaded_ts) = load(&path).unwrap();
        assert_eq!(loaded_ts, ts);
        assert_eq!(loaded.songs().len(), 1);
        assert_eq!(loaded.songs()[0].title, "Loved");

        let _ = fs::remove_dir_all(&dir);
    }
}
