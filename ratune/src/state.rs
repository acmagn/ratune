use std::collections::HashMap;
use std::time::Duration;

use ratune_subsonic::models::{Album, Artist, MusicFolder, Song};

// ── LoadingState ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub enum LoadingState<T> {
    #[default]
    NotLoaded,
    Loading,
    Loaded(T),
    Error(String),
}

// ── LibraryState ──────────────────────────────────────────────────────────────

/// Per-pane selection and lazy-loaded data cache for the library browser.
#[derive(Debug, Default)]
pub struct LibraryState {
    pub artists: LoadingState<Vec<Artist>>,
    pub selected_artist: Option<usize>,

    /// Albums keyed by artist ID.
    pub albums: HashMap<String, LoadingState<Vec<Album>>>,
    pub selected_album: Option<usize>,

    /// Songs keyed by album ID.
    pub tracks: HashMap<String, LoadingState<Vec<Song>>>,
    pub selected_track: Option<usize>,

    /// First visible row index in the artist column (filtered list coordinates).
    pub artists_scroll: usize,
    pub albums_scroll: usize,
    pub tracks_scroll: usize,
}

impl LibraryState {
    /// Keep row `sel` visible in a list of `len` rows with `visible` rows on screen.
    pub fn clamp_vertical_scroll(scroll: &mut usize, sel: usize, len: usize, visible: usize) {
        if len == 0 || visible == 0 {
            return;
        }
        let sel = sel.min(len - 1);
        let max_first = len.saturating_sub(visible);
        if *scroll > max_first {
            *scroll = max_first;
        }
        if sel < *scroll {
            *scroll = sel;
        } else if sel >= *scroll + visible {
            *scroll = sel + 1 - visible;
        }
    }

    /// The artist currently highlighted, if the artist list is loaded.
    pub fn current_artist(&self) -> Option<&Artist> {
        if let LoadingState::Loaded(artists) = &self.artists {
            self.selected_artist.and_then(|i| artists.get(i))
        } else {
            None
        }
    }

    /// The album currently highlighted for the selected artist, if loaded.
    pub fn current_album(&self) -> Option<&Album> {
        let artist_id = self.current_artist().map(|a| a.id.as_str())?;
        if let Some(LoadingState::Loaded(albums)) = self.albums.get(artist_id) {
            self.selected_album.and_then(|i| albums.get(i))
        } else {
            None
        }
    }

    /// The track currently highlighted for the selected album, if loaded.
    pub fn current_track(&self) -> Option<&Song> {
        let album_id = self.current_album().map(|a| a.id.as_str())?;
        if let Some(LoadingState::Loaded(songs)) = self.tracks.get(album_id) {
            self.selected_track.and_then(|i| songs.get(i))
        } else {
            None
        }
    }
}

// ── FolderBrowseState ─────────────────────────────────────────────────────────

/// Cached contents of one `getMusicDirectory` response.
#[derive(Debug, Clone)]
pub struct DirectoryListing {
    pub name: String,
    pub directories: Vec<(String, String)>,
    pub tracks: Vec<Song>,
}

/// One row in the folder preview pane (right column): subfolders first, then tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderPreviewRow {
    Dir(usize),
    Track(usize),
}

/// Build filtered preview rows for `listing` (directories then tracks).
pub fn folder_preview_rows(
    listing: &DirectoryListing,
    filter: Option<&str>,
) -> Vec<FolderPreviewRow> {
    let mut out = Vec::new();
    match filter {
        Some(q) => {
            let ql = q.to_lowercase();
            for (i, (_, name)) in listing.directories.iter().enumerate() {
                if name.to_lowercase().contains(&ql) {
                    out.push(FolderPreviewRow::Dir(i));
                }
            }
            for (i, s) in listing.tracks.iter().enumerate() {
                if s.title.to_lowercase().contains(&ql) {
                    out.push(FolderPreviewRow::Track(i));
                }
            }
        }
        None => {
            for i in 0..listing.directories.len() {
                out.push(FolderPreviewRow::Dir(i));
            }
            for i in 0..listing.tracks.len() {
                out.push(FolderPreviewRow::Track(i));
            }
        }
    }
    out
}

/// Left folder column row count inside a directory: `..` plus search-filtered subdirectories.
fn folder_left_visible_rows_len(listing: &DirectoryListing, filter: Option<&str>) -> usize {
    let sub = if let Some(q) = filter {
        listing
            .directories
            .iter()
            .filter(|(_, name)| name.to_lowercase().contains(q))
            .count()
    } else {
        listing.directories.len()
    };
    1 + sub
}

/// Default highlighted row: skip `..` when at least one subdirectory is visible.
pub fn folder_left_default_row(listing: &DirectoryListing, filter: Option<&str>) -> usize {
    let len = folder_left_visible_rows_len(listing, filter);
    if len >= 2 {
        1
    } else {
        0
    }
}

/// File/folder browser state (`[ui.browsetab] mode = "files"`).
#[derive(Debug, Default)]
pub struct FolderBrowseState {
    pub roots: LoadingState<Vec<MusicFolder>>,
    /// `(id, display name)` from the selected root into the current folder.
    pub path: Vec<(String, String)>,
    pub listings: std::collections::HashMap<String, LoadingState<DirectoryListing>>,
    pub selected_dir: Option<usize>,
    /// After path changes, set selection when the current folder listing finishes loading.
    pub folder_default_row_pending: bool,
    /// Listing id shown in the right-hand preview column (`browse_folder_listing` cache key).
    pub preview_dir_id: Option<String>,
    /// Selected row in the preview list (filtered dirs + tracks).
    pub preview_selected_row: usize,
    pub dirs_scroll: usize,
    pub tracks_scroll: usize,
}

impl FolderBrowseState {
    pub fn clamp_scroll(scroll: &mut usize, sel: usize, len: usize, visible: usize) {
        LibraryState::clamp_vertical_scroll(scroll, sel, len, visible);
    }

    pub fn current_dir_id(&self) -> Option<&str> {
        self.path.last().map(|(id, _)| id.as_str())
    }

    pub fn current_listing(&self) -> Option<&LoadingState<DirectoryListing>> {
        let id = self.current_dir_id()?;
        self.listings.get(id)
    }

    pub fn current_preview_track(&self, filter: Option<&str>) -> Option<&Song> {
        let pid = self.preview_dir_id.as_ref()?;
        let listing = match self.listings.get(pid)? {
            LoadingState::Loaded(l) => l,
            _ => return None,
        };
        let rows = folder_preview_rows(listing, filter);
        let row = rows.get(self.preview_selected_row)?;
        match row {
            FolderPreviewRow::Track(i) => listing.tracks.get(*i),
            FolderPreviewRow::Dir(_) => None,
        }
    }
}

// ── QueueState ────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct QueueState {
    pub songs: Vec<Song>,
    /// Index of the currently playing (or next-to-play) song.
    pub cursor: usize,
    /// Offset for list rendering (scroll).
    pub scroll: usize,
    /// Snapshot of the queue order taken just before the last shuffle.
    /// `None` means no unshuffle is available.
    pub pre_shuffle_order: Option<Vec<Song>>,
}

impl QueueState {
    pub fn push(&mut self, song: Song) {
        // Keep pre_shuffle_order in sync: it is the canonical "original order"
        // and must always reflect what the queue would look like un-shuffled.
        // Initialized lazily on first push so that Option::None means
        // "no songs have been added yet" rather than "unshuffle unavailable".
        match &mut self.pre_shuffle_order {
            Some(orig) => orig.push(song.clone()),
            None => self.pre_shuffle_order = Some(vec![song.clone()]),
        }
        self.songs.push(song);
    }

    pub fn current(&self) -> Option<&Song> {
        self.songs.get(self.cursor)
    }

    /// Advance to the next song. Returns true if there is a next song.
    pub fn next(&mut self) -> bool {
        if self.cursor + 1 < self.songs.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    /// Peek at the next song without advancing the cursor.
    pub fn peek_next(&self) -> Option<&Song> {
        self.songs.get(self.cursor + 1)
    }

    /// Move to the previous song. Returns true if there is a previous song.
    pub fn prev(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    /// Keep the playback cursor row visible in a list with `visible` rows (excluding borders).
    /// Does not force the cursor to the top of the viewport; scroll only moves when needed.
    pub fn scroll_clamp_cursor_visible(&mut self, visible: usize) {
        let len = self.songs.len();
        if len == 0 || visible == 0 {
            return;
        }
        let c = self.cursor.min(len - 1);
        let max_first = len.saturating_sub(visible);
        if self.scroll > max_first {
            self.scroll = max_first;
        }
        if c < self.scroll {
            self.scroll = c;
        } else if c >= self.scroll + visible {
            self.scroll = c + 1 - visible;
        }
    }

    /// Insert `incoming` at the front of the queue in order; advances `cursor` so the
    /// currently playing track index still refers to the same song.
    pub fn prepend_songs(&mut self, incoming: Vec<Song>) {
        if incoming.is_empty() {
            return;
        }
        if self.songs.is_empty() {
            for s in incoming {
                self.push(s);
            }
            return;
        }
        let n = incoming.len();
        match &mut self.pre_shuffle_order {
            Some(orig) => {
                for s in incoming.iter().rev() {
                    orig.insert(0, s.clone());
                }
            }
            None => {
                self.pre_shuffle_order =
                    Some(incoming.iter().chain(self.songs.iter()).cloned().collect());
            }
        }
        for s in incoming.into_iter().rev() {
            self.songs.insert(0, s);
        }
        self.cursor += n;
    }

    /// Remove the song at `idx`, adjusting `cursor` and `pre_shuffle_order`.
    pub fn remove_at(&mut self, idx: usize) -> Option<Song> {
        if idx >= self.songs.len() {
            return None;
        }
        let removed = self.songs.remove(idx);
        if let Some(orig) = &mut self.pre_shuffle_order {
            if let Some(orig_idx) = orig.iter().position(|s| s.id == removed.id) {
                orig.remove(orig_idx);
            }
        }
        if self.songs.is_empty() {
            self.cursor = 0;
            self.scroll = 0;
        } else if idx < self.cursor {
            self.cursor -= 1;
        } else if idx == self.cursor && self.cursor >= self.songs.len() {
            self.cursor = self.songs.len() - 1;
        }
        Some(removed)
    }
}

/// Pending confirmation for destructive / expensive actions (outside playlist overlay).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalConfirm {
    LibraryIndexRefresh,
    /// Append every track in the on-disk metadata index to the queue (after y/n).
    LibraryIndexAppendQueue,
    /// Fetch the full library from the server and append it to the queue (after y/n).
    LibraryServerAppendQueue,
}

// ── PlaylistOverlay ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub enum PlaylistFocus {
    #[default]
    List,
    Tracks,
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeletePlaylist { id: String, name: String },
}

#[derive(Debug, Clone, Default)]
pub enum PlaylistInputMode {
    #[default]
    Normal,
    Creating {
        buffer: String,
    },
    Renaming {
        buffer: String,
        playlist_id: String,
    },
    Confirming {
        action: ConfirmAction,
    },
}

#[derive(Debug)]
pub struct PlaylistOverlay {
    pub visible: bool,
    pub playlists: LoadingState<Vec<ratune_subsonic::Playlist>>,
    pub selected_playlist_index: usize,
    pub tracks: LoadingState<Vec<ratune_subsonic::Song>>,
    pub selected_track_index: usize,
    pub focus: PlaylistFocus,
    pub loaded_playlist_id: Option<String>,
    pub input_mode: PlaylistInputMode,
}

impl Default for PlaylistOverlay {
    fn default() -> Self {
        Self {
            visible: false,
            playlists: LoadingState::NotLoaded,
            selected_playlist_index: 0,
            tracks: LoadingState::NotLoaded,
            selected_track_index: 0,
            focus: PlaylistFocus::List,
            loaded_playlist_id: None,
            input_mode: PlaylistInputMode::Normal,
        }
    }
}

// ── PlaybackState ─────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct PlaybackState {
    pub current_song: Option<Song>,
    pub elapsed: Duration,
    pub total: Option<Duration>,
    pub paused: bool,
    /// True once a `PlayUrl` has been sent to the engine for this track.
    /// False after restore (current_song is set but engine has nothing loaded).
    pub player_loaded: bool,
}

#[cfg(test)]
mod queue_tests {
    use super::QueueState;
    use ratune_subsonic::Song;

    fn song(id: &str) -> Song {
        Song {
            id: id.to_string(),
            title: id.to_string(),
            album: None,
            artist: None,
            album_id: None,
            artist_id: None,
            track: None,
            disc_number: None,
            year: None,
            genre: None,
            cover_art: None,
            duration: None,
            bit_rate: None,
            content_type: None,
            suffix: None,
            size: None,
            path: None,
            starred: None,
        }
    }

    #[test]
    fn remove_before_cursor_shifts_cursor_down() {
        let mut q = QueueState::default();
        q.push(song("a"));
        q.push(song("b"));
        q.push(song("c"));
        q.cursor = 2;
        assert!(q.remove_at(0).is_some());
        assert_eq!(q.songs.len(), 2);
        assert_eq!(q.cursor, 1);
        assert_eq!(q.songs[1].id, "c");
    }

    #[test]
    fn remove_at_cursor_keeps_next_track_selected() {
        let mut q = QueueState::default();
        q.push(song("a"));
        q.push(song("b"));
        q.push(song("c"));
        q.cursor = 1;
        assert!(q.remove_at(1).is_some());
        assert_eq!(q.cursor, 1);
        assert_eq!(q.songs[q.cursor].id, "c");
    }

    #[test]
    fn remove_last_track_empties_queue() {
        let mut q = QueueState::default();
        q.push(song("a"));
        assert!(q.remove_at(0).is_some());
        assert!(q.songs.is_empty());
        assert_eq!(q.cursor, 0);
    }

    #[test]
    fn remove_syncs_pre_shuffle_order() {
        let mut q = QueueState::default();
        q.push(song("a"));
        q.push(song("b"));
        q.push(song("c"));
        assert!(q.remove_at(1).is_some());
        let orig = q.pre_shuffle_order.as_ref().unwrap();
        assert_eq!(orig.len(), 2);
        assert_eq!(orig[0].id, "a");
        assert_eq!(orig[1].id, "c");
    }
}
