#[derive(Debug, Clone)]
pub enum Direction {
    Up,
    Down,
    Top,    // gg (vim-style)
    Bottom, // G
    PageUp,
    PageDown,
}

use std::time::Duration;

#[derive(Debug, Clone)]
pub enum Action {
    Navigate(Direction),
    Select,
    Back,
    /// Cycle tabs forward: Home → Browser → Now Playing → Home (Tab key)
    SwitchTab,
    /// Cycle tabs backward: Home → Now Playing → Browser → Home (Backtick / Shift+Tab)
    SwitchTabReverse,
    /// Jump directly to Home tab (key '1')
    GoToHome,
    /// Jump directly to Browser tab (key '2')
    GoToBrowser,
    /// Toggle Browse tab between artist columns and folder layout (requires config).
    ToggleBrowserFolder,
    /// Jump directly to NowPlaying tab (key '3')
    GoToNowPlaying,
    /// Open or close the internet radio station picker (default: Shift+R).
    ToggleRadioPicker,
    /// Play the selected station from the radio picker and close it.
    RadioPickerSelect,
    /// Close the radio picker without playing.
    RadioPickerCancel,
    FocusLeft,
    FocusRight,
    AddToQueue,
    AddAllToQueue,
    /// Browser: replace queue with the **current album** (selected album + loaded tracks) and play.
    AddAllToQueueReplaceAlbum,
    /// Browser: replace queue with **all tracks for the current artist** (API fetch) and play.
    AddAllToQueueReplaceArtist,
    /// Browser: add all tracks — insert at the front of the queue.
    AddAllToQueuePrepend,
    PlayPause,
    NextTrack,
    PrevTrack,
    VolumeUp,
    VolumeDown,
    ClearQueue,
    /// Remove the highlighted track from the queue (Now Playing tab).
    RemoveFromQueue,
    Shuffle,
    Unshuffle,
    /// Toggle whether the queue loops after the last track (↻ control).
    ToggleQueueLoop,
    /// Toggle Now Playing pane focus between live radio and library queue.
    ToggleNpPaneFocus,
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
    /// Toggle favorite (star) status for the focused or playing track.
    ToggleFavorite,
    /// Toggle the favorites (starred) browser overlay.
    ToggleFavoritesOverlay,
    /// Favorites overlay: scroll up.
    FavoritesScrollUp,
    /// Favorites overlay: scroll down.
    FavoritesScrollDown,
    /// Favorites overlay: focus category list.
    FavoritesFocusCategories,
    /// Favorites overlay: focus items list.
    FavoritesFocusItems,
    /// Favorites overlay: replace queue and play (category or item).
    FavoritesPlay,
    /// Favorites overlay: append to queue (category or item).
    FavoritesAppend,
    /// Scroll the help popup up one line.
    HelpScrollUp,
    /// Scroll the help popup down one line.
    HelpScrollDown,
    /// Move to the next section on the Home tab (RecentAlbums → RecentTracks → TopArtists → Rediscover).
    HomeSectionNext,
    /// Move to the previous section on the Home tab.
    HomeSectionPrev,
    /// Refresh Home tab data (re-rolls rediscover suggestions).
    HomeRefresh,
    /// Refresh internet radio stations while the picker is open.
    RadioRefresh,
    /// Open the new-station form on the Radio tab.
    RadioCreate,
    /// Open the edit-station form for the selected station.
    RadioEdit,
    /// Prompt to delete the selected station.
    RadioDelete,
    /// Move to the next field in the radio station form.
    RadioFieldNext,
    /// Move to the previous field in the radio station form.
    RadioFieldPrev,
    /// Submit the radio station form.
    RadioInputConfirm,
    /// Cancel the radio station form or delete prompt.
    RadioInputCancel,
    /// Feed a character into the focused radio form field.
    RadioInputChar(char),
    /// Confirm deleting a radio station.
    RadioConfirmYes,
    /// Cancel deleting a radio station.
    RadioConfirmNo,
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
    /// Confirm a pending full library index refresh.
    ConfirmLibraryIndexRefresh,
    /// Propose appending all indexed library tracks to the queue (shows y/n first).
    LibraryIndexAppendQueue,
    /// Confirm pending append of the full metadata index to the queue.
    ConfirmLibraryIndexAppendQueue,
    /// Confirm pending append of the full server library (non-index) to the queue.
    ConfirmLibraryServerAppendQueue,
    /// Dismiss a global confirmation prompt (e.g. library refresh).
    CancelGlobalConfirm,
    /// Check server connectivity now (optional keybind; periodic check uses `[server].connection_check_interval_secs`).
    CheckConnection,
    Quit,
    None,
}
