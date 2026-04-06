#[derive(Debug, Clone)]
pub enum Direction {
    Up,
    Down,
    Top,    // g
    Bottom, // G
}

use std::time::Duration;

#[derive(Debug, Clone)]
pub enum Action {
    Navigate(Direction),
    Select,
    Back,
    /// Cycle tabs forward: Home → Browser → NowPlaying → Home (Tab key)
    SwitchTab,
    /// Cycle tabs backward: Home → NowPlaying → Browser → Home (Backtick / Shift+Tab)
    SwitchTabReverse,
    /// Jump directly to Home tab (key '1')
    GoToHome,
    /// Jump directly to Browser tab (key '2')
    GoToBrowser,
    /// Jump directly to NowPlaying tab (key '3')
    GoToNowPlaying,
    FocusLeft,
    FocusRight,
    AddToQueue,
    AddAllToQueue,
    PlayPause,
    NextTrack,
    PrevTrack,
    VolumeUp,
    VolumeDown,
    ClearQueue,
    Shuffle,
    Unshuffle,
    SeekForward,
    SeekBackward,
    /// Seek to an exact position (used by progress-bar clicks).
    SeekTo(Duration),
    SearchStart,
    SearchInput(char),
    SearchBackspace,
    SearchConfirm,
    SearchCancel,
    /// Toggle dynamic accent colour extraction from album art.
    ToggleDynamicTheme,
    /// Toggle the lyrics overlay on the NowPlaying tab.
    ToggleLyrics,
    /// Toggle the spectrum visualizer overlay on the NowPlaying tab.
    ToggleVisualizer,
    /// Toggle the keybind reference popup.
    ToggleHelp,
    /// Move to the next section on the Home tab (RecentAlbums → RecentTracks → TopArtists → Rediscover).
    HomeSectionNext,
    /// Move to the previous section on the Home tab.
    HomeSectionPrev,
    /// Refresh Home tab data (re-rolls rediscover suggestions).
    HomeRefresh,
    /// Navigate the art strip left (decrement selected album).
    HomeAlbumLeft,
    /// Navigate the art strip right (increment selected album).
    HomeAlbumRight,
    /// Add the selected album (strip) to queue, replacing existing queue.
    #[allow(dead_code)]
    HomeAlbumPlay,
    /// Append the selected album (strip) to queue without clearing.
    HomeAlbumAddToQueue,
    /// Toggle the playlist browser overlay.
    TogglePlaylistOverlay,
    /// Scroll up within the playlist overlay (list or tracks pane).
    PlaylistScrollUp,
    /// Scroll down within the playlist overlay (list or tracks pane).
    PlaylistScrollDown,
    /// Move focus to the tracks pane of the playlist overlay.
    PlaylistFocusTracks,
    /// Move focus back to the playlist list pane of the overlay.
    PlaylistFocusList,
    /// Replace the queue with all tracks from the selected playlist and play.
    PlaylistPlayAll,
    /// Append all tracks from the selected playlist to the queue.
    PlaylistAppendAll,
    /// Replace the queue with the highlighted track and play.
    PlaylistPlayTrack,
    /// Append the highlighted track to the queue.
    PlaylistAppendTrack,
    /// Create a new playlist (opens the name-input prompt).
    PlaylistCreate,
    /// Delete the currently selected playlist (opens confirmation prompt).
    PlaylistDelete,
    /// Rename the currently selected playlist (opens the rename-input prompt).
    PlaylistRename,
    /// Remove the highlighted track from the current playlist.
    PlaylistRemoveTrack,
    /// Open the playlist picker to add the focused browser track to a playlist.
    BrowserAddToPlaylist,
    /// Confirm selection in the playlist picker.
    PlaylistPickerSelect,
    /// Cancel and close the playlist picker.
    PlaylistPickerCancel,
    /// Scroll up in the playlist picker.
    PlaylistPickerScrollUp,
    /// Scroll down in the playlist picker.
    PlaylistPickerScrollDown,
    /// Confirm the current text-input field (create / rename).
    PlaylistInputConfirm,
    /// Cancel the current text-input field.
    PlaylistInputCancel,
    /// Feed a character into the active text-input buffer.
    PlaylistInputChar(char),
    /// Confirm the yes/no confirmation prompt.
    PlaylistConfirmYes,
    /// Decline the yes/no confirmation prompt.
    PlaylistConfirmNo,
    /// Open the fzf (or `sk`) track picker using the local metadata index.
    LibraryFzfPicker,
    /// Force a full refresh of the metadata index from Subsonic.
    LibraryIndexRefresh,
    Quit,
    None,
}
