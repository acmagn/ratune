use std::collections::HashMap;
use std::time::Duration;

use ratune_subsonic::models::{Album, Artist, Song};

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
