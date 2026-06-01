use std::time::Duration;

/// Which scrobbling rules to apply when deciding if a listen counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdProfile {
    /// Ratune local history and Subsonic server scrobble: min(50% of duration, 30 s).
    Local,
    /// Last.fm / Libre.fm (Audioscrobbler): min(50% of duration, 4 min).
    Audioscrobbler,
}

/// Minimum playback time before a listen is recorded for `profile`.
///
/// When track duration is unknown, both profiles fall back to 30 seconds.
pub fn play_threshold(total: Option<Duration>, profile: ThresholdProfile) -> Duration {
    let cap = match profile {
        ThresholdProfile::Local => Duration::from_secs(30),
        ThresholdProfile::Audioscrobbler => Duration::from_secs(4 * 60),
    };
    match total {
        Some(dur) => {
            let half = dur / 2;
            half.min(cap)
        }
        None => Duration::from_secs(30),
    }
}

/// Audioscrobbler services ignore tracks that are 30 seconds or shorter.
pub fn audioscrobbler_eligible(total: Option<Duration>) -> bool {
    total.is_none_or(|d| d > Duration::from_secs(30))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_caps_at_thirty_seconds() {
        let ten_min = Some(Duration::from_secs(600));
        assert_eq!(
            play_threshold(ten_min, ThresholdProfile::Local),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn audioscrobbler_caps_at_four_minutes() {
        let ten_min = Some(Duration::from_secs(600));
        assert_eq!(
            play_threshold(ten_min, ThresholdProfile::Audioscrobbler),
            Duration::from_secs(240)
        );
    }

    #[test]
    fn half_duration_when_shorter_than_cap() {
        let three_min = Some(Duration::from_secs(180));
        assert_eq!(
            play_threshold(three_min, ThresholdProfile::Audioscrobbler),
            Duration::from_secs(90)
        );
    }

    #[test]
    fn audioscrobbler_rejects_short_tracks() {
        assert!(!audioscrobbler_eligible(Some(Duration::from_secs(30))));
        assert!(audioscrobbler_eligible(Some(Duration::from_secs(31))));
        assert!(audioscrobbler_eligible(None));
    }
}
