//! Audioscrobbler scrobbling for Last.fm and Libre.fm, plus shared play-threshold logic.

pub mod auth;
pub mod lastfm;
pub mod threshold;
pub mod track;

pub use auth::{AuthClient, AuthSession};
pub use lastfm::{AudioscrobblerClient, ScrobbleService};
pub use threshold::{
    audioscrobbler_eligible, play_threshold, AudioscrobblerRules, ListenThreshold,
};
pub use track::TrackInfo;
