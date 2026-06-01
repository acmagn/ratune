//! Last.fm / Libre.fm Audioscrobbler API v2.0 client.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;

use crate::track::TrackInfo;

/// Which Audioscrobbler-compatible API endpoint to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrobbleService {
    LastFm,
    LibreFm,
}

impl ScrobbleService {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lastfm" | "last.fm" | "last_fm" => Some(Self::LastFm),
            "librefm" | "libre.fm" | "libre_fm" => Some(Self::LibreFm),
            _ => None,
        }
    }

    pub(crate) fn api_base(&self) -> &'static str {
        match self {
            Self::LastFm => "https://ws.audioscrobbler.com/2.0/",
            Self::LibreFm => "https://libre.fm/2.0/",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::LastFm => "Last.fm",
            Self::LibreFm => "Libre.fm",
        }
    }
}

/// Authenticated client for `track.updateNowPlaying` and `track.scrobble`.
#[derive(Debug, Clone)]
pub struct AudioscrobblerClient {
    http: Client,
    api_key: String,
    api_secret: String,
    session_key: String,
    service: ScrobbleService,
}

impl AudioscrobblerClient {
    pub fn new(
        service: ScrobbleService,
        api_key: String,
        api_secret: String,
        session_key: String,
    ) -> Self {
        Self {
            http: Client::new(),
            api_key,
            api_secret,
            session_key,
            service,
        }
    }

    pub fn service(&self) -> ScrobbleService {
        self.service
    }

    /// Announce the currently playing track (does not count as a scrobble).
    pub async fn update_now_playing(&self, track: &TrackInfo) -> Result<()> {
        let mut params = self.base_params("track.updateNowPlaying");
        params.insert("artist".into(), track.artist.clone());
        params.insert("track".into(), track.title.clone());
        if let Some(ref album) = track.album {
            if !album.is_empty() {
                params.insert("album".into(), album.clone());
            }
        }
        if let Some(n) = track.track_number {
            params.insert("trackNumber".into(), n.to_string());
        }
        if let Some(d) = track.duration_secs {
            params.insert("duration".into(), d.to_string());
        }
        self.post_signed(params).await
    }

    /// Submit a completed listen. `timestamp` is Unix seconds when playback started.
    pub async fn scrobble(&self, track: &TrackInfo, timestamp: i64) -> Result<()> {
        let mut params = self.base_params("track.scrobble");
        params.insert("artist[0]".into(), track.artist.clone());
        params.insert("track[0]".into(), track.title.clone());
        params.insert("timestamp[0]".into(), timestamp.to_string());
        if let Some(ref album) = track.album {
            if !album.is_empty() {
                params.insert("album[0]".into(), album.clone());
            }
        }
        if let Some(n) = track.track_number {
            params.insert("trackNumber[0]".into(), n.to_string());
        }
        if let Some(d) = track.duration_secs {
            params.insert("duration[0]".into(), d.to_string());
        }
        self.post_signed(params).await
    }

    fn base_params(&self, method: &str) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        params.insert("method".into(), method.into());
        params.insert("api_key".into(), self.api_key.clone());
        params.insert("sk".into(), self.session_key.clone());
        params.insert("format".into(), "json".into());
        params
    }

    async fn post_signed(&self, mut params: BTreeMap<String, String>) -> Result<()> {
        let sig = api_sig(&params, &self.api_secret);
        params.insert("api_sig".into(), sig);

        let resp = self
            .http
            .post(self.service.api_base())
            .form(&params)
            .send()
            .await
            .with_context(|| format!("POST {}", self.service.api_base()))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .with_context(|| format!("parsing {} response", self.service.display_name()))?;

        if let Some(err) = body.get("error") {
            let code = err.as_i64().unwrap_or(-1);
            let message = body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            bail!(
                "{} API error {code}: {message}",
                self.service.display_name()
            );
        }

        if !status.is_success() {
            bail!(
                "{} API HTTP {status}",
                self.service.display_name()
            );
        }

        Ok(())
    }
}

/// MD5 signature required for all authenticated Last.fm POST requests.
pub(crate) fn api_sig(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut concat = String::new();
    for (k, v) in params {
        if k == "format" || k == "api_sig" {
            continue;
        }
        concat.push_str(k);
        concat.push_str(v);
    }
    concat.push_str(secret);
    format!("{:x}", md5::compute(concat.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_sig_is_deterministic_and_excludes_format() {
        let mut params = BTreeMap::new();
        params.insert("api_key".into(), "myApiKey".into());
        params.insert("method".into(), "auth.getSession".into());
        params.insert("token".into(), "myToken".into());
        params.insert("format".into(), "json".into());
        let sig = api_sig(&params, "mySharedSecret");
        assert_eq!(sig, "ad652a8ce50787750507e5686a235f01");
        assert_eq!(sig, api_sig(&params, "mySharedSecret"));
    }

    #[test]
    fn parses_service_names() {
        assert_eq!(ScrobbleService::parse("Last.fm"), Some(ScrobbleService::LastFm));
        assert_eq!(ScrobbleService::parse("librefm"), Some(ScrobbleService::LibreFm));
        assert!(ScrobbleService::parse("spotify").is_none());
    }
}
