pub mod client;
pub mod error;
pub mod models;

pub use client::{
    fetch_all_library_songs, fetch_all_library_songs_with_options, fetch_library,
    fetch_songs_for_artist, FetchLibraryOptions, StarItemType, SubsonicClient, DEFAULT_SERVER_URL,
};
pub use error::{is_auth_failure, SubsonicError, AUTH_ERROR_CODE};
pub use models::{
    format_user_rating, format_user_rating_with_glyphs, format_user_rating_with_options,
    music_library_root_cache_key, parse_music_library_root_folder_id, user_rating_mpris, Album,
    Artist, ArtistIndex, Artists, DirectoryChild, Indexes, InternetRadioStation, LyricLine,
    MusicDirectory, MusicFolder, Playlist, PlaylistDetail, ScanStatus, SearchResult3, Song,
    Starred2, SubsonicLibrary, DEFAULT_RATING_BRACKET_CLOSE, DEFAULT_RATING_BRACKET_OPEN,
    DEFAULT_RATING_STAR_EMPTY, DEFAULT_RATING_STAR_FILLED, MUSIC_FOLDER_ROOT_ID_PREFIX,
};
