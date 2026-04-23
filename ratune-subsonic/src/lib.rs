pub mod client;
pub mod error;
pub mod models;

pub use client::{
    fetch_all_library_songs, fetch_all_library_songs_with_options, fetch_library,
    fetch_songs_for_artist, FetchLibraryOptions, SubsonicClient, DEFAULT_SERVER_URL,
};
pub use error::SubsonicError;
pub use models::{
    Album, Artist, ArtistIndex, Artists, LyricLine, Playlist, PlaylistDetail, ScanStatus,
    SearchResult3, Song, SubsonicLibrary,
};
