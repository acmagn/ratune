use std::time::Duration;

/// When a listen counts: min(fraction of track length, max_listen cap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenThreshold {
    /// Whole-number percent of track duration (1–100). Default convention: 50.
    pub min_percent: u8,
    /// Upper cap on listen time before counting. Default varies by use case.
    pub max_listen: Duration,
}

impl ListenThreshold {
    pub const fn local_default() -> Self {
        Self {
            min_percent: 50,
            max_listen: Duration::from_secs(30),
        }
    }

    pub const fn audioscrobbler_default() -> Self {
        Self {
            min_percent: 50,
            max_listen: Duration::from_secs(4 * 60),
        }
    }
}

impl Default for ListenThreshold {
    fn default() -> Self {
        Self::local_default()
    }
}

/// Last.fm / Libre.fm eligibility and listen threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioscrobblerRules {
    pub listen: ListenThreshold,
    /// Tracks must be longer than this to scrobble (Last.fm convention: 30 s).
    pub min_track_length: Duration,
}

impl Default for AudioscrobblerRules {
    fn default() -> Self {
        Self {
            listen: ListenThreshold::audioscrobbler_default(),
            min_track_length: Duration::from_secs(30),
        }
    }
}

/// Minimum playback time before a listen is recorded.
///
/// When track duration is unknown, falls back to `rules.max_listen`.
pub fn play_threshold(total: Option<Duration>, rules: ListenThreshold) -> Duration {
    let percent = rules.min_percent.clamp(1, 100);
    let cap = rules.max_listen;
    match total {
        Some(dur) => {
            let secs = dur.as_secs().saturating_mul(u64::from(percent)) / 100;
            Duration::from_secs(secs).min(cap)
        }
        None => cap,
    }
}

/// Whether a track is long enough to submit to Audioscrobbler services.
pub fn audioscrobbler_eligible(total: Option<Duration>, rules: AudioscrobblerRules) -> bool {
    total.is_none_or(|d| d > rules.min_track_length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_caps_at_thirty_seconds() {
        let ten_min = Some(Duration::from_secs(600));
        assert_eq!(
            play_threshold(ten_min, ListenThreshold::local_default()),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn audioscrobbler_caps_at_four_minutes() {
        let ten_min = Some(Duration::from_secs(600));
        assert_eq!(
            play_threshold(ten_min, ListenThreshold::audioscrobbler_default()),
            Duration::from_secs(240)
        );
    }

    #[test]
    fn half_duration_when_shorter_than_cap() {
        let three_min = Some(Duration::from_secs(180));
        assert_eq!(
            play_threshold(three_min, ListenThreshold::audioscrobbler_default()),
            Duration::from_secs(90)
        );
    }

    #[test]
    fn audioscrobbler_rejects_short_tracks() {
        let rules = AudioscrobblerRules::default();
        assert!(!audioscrobbler_eligible(
            Some(Duration::from_secs(30)),
            rules
        ));
        assert!(audioscrobbler_eligible(
            Some(Duration::from_secs(31)),
            rules
        ));
        assert!(audioscrobbler_eligible(None, rules));
    }

    #[test]
    fn custom_percent() {
        let rules = ListenThreshold {
            min_percent: 75,
            max_listen: Duration::from_secs(600),
        };
        assert_eq!(
            play_threshold(Some(Duration::from_secs(200)), rules),
            Duration::from_secs(150)
        );
    }
}
