//! ncmpcpp-inspired format strings for the now-playing bar (`%t`, `%a`, … and `$1`–`$8` colors).

use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Context for resolving `%` placeholders.
pub struct NowPlayingContext<'a> {
    pub song: Option<&'a playterm_subsonic::Song>,
    pub elapsed: Duration,
    pub total: Option<Duration>,
    /// Reserved for a future `%` code (pause / state).
    #[allow(dead_code)]
    pub paused: bool,
    /// Output volume 0–100 (same as `[player]` / UI).
    pub volume_percent: u8,
    /// Sum of `duration` for all queue tracks that have it (`None` if the queue is empty).
    pub queue_total_duration_secs: Option<u64>,
    /// 1-based index of the current track in the queue, and total track count.
    pub queue_position: Option<(usize, usize)>,
}

fn palette(theme: &Theme, accent: Color) -> [Color; 8] {
    [
        accent,
        theme.foreground,
        theme.dimmed,
        accent,
        theme.border_active,
        theme.dimmed,
        theme.foreground,
        theme.border,
    ]
}

/// Format seconds as `H:MM:SS` if ≥1h, else `M:SS`.
pub fn format_clock_duration_secs(total_secs: u64) -> String {
    if total_secs >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            total_secs / 3600,
            (total_secs % 3600) / 60,
            total_secs % 60
        )
    } else {
        format!("{}:{:02}", total_secs / 60, total_secs % 60)
    }
}

fn placeholder_value(key: char, ctx: &NowPlayingContext<'_>) -> String {
    let s = ctx.song;
    match key {
        't' => s.map(|x| x.title.as_str()).unwrap_or("").to_string(),
        'a' => s.and_then(|x| x.artist.as_deref()).unwrap_or("").to_string(),
        'b' => s.and_then(|x| x.album.as_deref()).unwrap_or("").to_string(),
        'y' => s.and_then(|x| x.year).map(|y| y.to_string()).unwrap_or_default(),
        'n' => s
            .and_then(|x| x.track)
            .map(|n| format!("{n:02}"))
            .unwrap_or_default(),
        'l' => s
            .and_then(|x| x.duration)
            .map(|d| format!("{}:{:02}", d / 60, d % 60))
            .unwrap_or_else(|| "--:--".into()),
        'g' => s.and_then(|x| x.genre.as_deref()).unwrap_or("").to_string(),
        'f' => s
            .and_then(|x| x.path.as_deref())
            .map(|p| std::path::Path::new(p))
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        'D' => s
            .and_then(|x| x.path.as_deref())
            .map(|p| std::path::Path::new(p))
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        // Album artist — Subsonic `Song` has no separate field yet.
        'A' => String::new(),
        'c' | 'p' => String::new(),
        'q' => s.and_then(format_quality).unwrap_or_default(),
        'e' => {
            let e = ctx.elapsed.as_secs();
            format!("{}:{:02}", e / 60, e % 60)
        }
        // Total playback time (buffer duration). Uppercase `T` / `E`.
        'E' | 'T' => ctx
            .total
            .map(|t| {
                let ts = t.as_secs();
                format!("{}:{:02}", ts / 60, ts % 60)
            })
            .unwrap_or_else(|| "--:--".into()),
        // Playlist / queue (not file tags).
        'P' => ctx
            .queue_total_duration_secs
            .map(format_clock_duration_secs)
            .unwrap_or_else(|| "--:--".into()),
        'i' => ctx
            .queue_position
            .map(|(x, _)| x.to_string())
            .unwrap_or_default(),
        'j' => ctx
            .queue_position
            .map(|(_, y)| y.to_string())
            .unwrap_or_default(),
        'v' => format!("{}", ctx.volume_percent),
        'K' => s
            .and_then(|x| x.bit_rate)
            .map(|br| format!("{br}"))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn format_quality(song: &playterm_subsonic::Song) -> Option<String> {
    let lossless = song.suffix.as_deref().map(|s| {
        matches!(
            s.to_lowercase().as_str(),
            "flac" | "wav" | "alac" | "ape" | "aiff"
        )
    })?;

    if lossless {
        return Some(song.suffix.as_deref().unwrap_or("").to_uppercase());
    }
    song.bit_rate.map(|br| format!("{br}kbps"))
}

/// Parse one line: `%` tags, `$1`–`$8` colors, `$9` reset, `$b`/`$/b`, `$u`/`$/u`, `$r`/`$/r`.
pub fn format_now_playing_line(
    template: &str,
    ctx: &NowPlayingContext<'_>,
    theme: &Theme,
    accent: Color,
) -> Line<'static> {
    let palette = palette(theme, accent);
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default().fg(theme.foreground);
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            if chars.peek() == Some(&'%') {
                chars.next();
                spans.push(Span::styled("%".to_string(), style));
                continue;
            }
            if let Some(k) = chars.next() {
                let v = placeholder_value(k, ctx);
                spans.push(Span::styled(v, style));
            }
            continue;
        }
        if c == '$' {
            if let Some(&next) = chars.peek() {
                if next == '/' {
                    chars.next();
                    if let Some(end) = chars.next() {
                        match end {
                            'b' => {
                                style = style.remove_modifier(Modifier::BOLD);
                            }
                            'u' => {
                                style = style.remove_modifier(Modifier::UNDERLINED);
                            }
                            'r' => {
                                style = style.remove_modifier(Modifier::REVERSED);
                            }
                            _ => {
                                spans.push(Span::styled("$/".to_string() + &end.to_string(), style));
                            }
                        }
                    }
                    continue;
                }
                if let Some(d) = next.to_digit(10) {
                    chars.next();
                    if (1..=8).contains(&d) {
                        style = style.fg(palette[(d as usize) - 1]);
                    } else if d == 9 {
                        style = style.fg(theme.foreground);
                    }
                    continue;
                }
                chars.next();
                match next {
                    'b' => {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    'u' => {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }
                    'r' => {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    _ => {
                        spans.push(Span::styled(format!("${next}"), style));
                    }
                }
                continue;
            }
            spans.push(Span::styled("$", style));
            continue;
        }
        spans.push(Span::styled(c.to_string(), style));
    }

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_title() {
        let theme = Theme::from_section(&crate::config::ThemeSection::default());
        let song = playterm_subsonic::Song {
            id: "1".into(),
            title: "Hello".into(),
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
        };
        let ctx = NowPlayingContext {
            song: Some(&song),
            elapsed: Duration::ZERO,
            total: None,
            paused: false,
            volume_percent: 70,
            queue_total_duration_secs: None,
            queue_position: None,
        };
        let line = format_now_playing_line("%t", &ctx, &theme, theme.accent);
        let s: String = line.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.contains("Hello"));
    }

    #[test]
    fn queue_and_bitrate_placeholders() {
        let theme = Theme::from_section(&crate::config::ThemeSection::default());
        let song = playterm_subsonic::Song {
            id: "1".into(),
            title: "Hello".into(),
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
            bit_rate: Some(320),
            content_type: None,
            suffix: None,
            size: None,
            path: None,
            starred: None,
        };
        let ctx = NowPlayingContext {
            song: Some(&song),
            elapsed: Duration::ZERO,
            total: None,
            paused: false,
            volume_percent: 42,
            queue_total_duration_secs: Some(125),
            queue_position: Some((2, 5)),
        };
        let line = format_now_playing_line("%K vol %v %P %i/%j", &ctx, &theme, theme.accent);
        let s: String = line.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.contains("320"));
        assert!(s.contains("42"));
        assert!(s.contains("2:05"));
        assert!(s.contains("2/5"));
    }

    #[test]
    fn bitrate_placeholder_empty_when_missing() {
        let theme = Theme::from_section(&crate::config::ThemeSection::default());
        let song = playterm_subsonic::Song {
            id: "1".into(),
            title: "Hello".into(),
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
        };
        let ctx = NowPlayingContext {
            song: Some(&song),
            elapsed: Duration::ZERO,
            total: None,
            paused: false,
            volume_percent: 0,
            queue_total_duration_secs: None,
            queue_position: None,
        };
        let line = format_now_playing_line("%K", &ctx, &theme, theme.accent);
        let s: String = line.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.trim().is_empty());
    }
}
