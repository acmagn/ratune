/// Configurable keybindings loaded from `[keybinds]` in config.toml.
///
/// Every bind has a default that matches the previous hardcoded behaviour.
/// Unset config fields simply fall back to the default.
use crossterm::event::{KeyCode, KeyModifiers};

use crate::config::KeybindsSection;

// ── KeySpec ───────────────────────────────────────────────────────────────────

/// A single key combination (code + optional modifiers).
#[derive(Debug, Clone)]
pub struct KeySpec {
    pub code:      KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeySpec {
    fn new(code: KeyCode) -> Self {
        Self { code, modifiers: KeyModifiers::empty() }
    }

    /// Returns true when `(code, mods)` matches this spec.
    ///
    /// Shift+letter chords are stored as lowercase `char` + [`SHIFT`]. Many terminals
    /// send an uppercase letter plus SHIFT instead; those are normalized here.
    pub fn matches(&self, code: KeyCode, mods: KeyModifiers) -> bool {
        if self.code == KeyCode::BackTab {
            return code == KeyCode::BackTab;
        }
        // Some terminals send `J` with no modifier bit for Shift+j.
        if let (KeyCode::Char(sc), KeyCode::Char(kc)) = (self.code, code) {
            if self.modifiers == KeyModifiers::SHIFT
                && sc.is_ascii_lowercase()
                && mods.is_empty()
                && kc.is_ascii_uppercase()
                && kc.to_ascii_lowercase() == sc
            {
                return true;
            }
        }
        let (code, mods) = match code {
            KeyCode::Char(k)
                if k.is_ascii_uppercase()
                    && k.is_ascii_alphabetic()
                    && mods.intersects(KeyModifiers::SHIFT) =>
            {
                (KeyCode::Char(k.to_ascii_lowercase()), mods)
            }
            c => (c, mods),
        };
        if self.code != code {
            return false;
        }
        if self.modifiers.is_empty() {
            let mask = KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER
                | KeyModifiers::HYPER
                | KeyModifiers::META
                | KeyModifiers::SHIFT;
            return !mods.intersects(mask);
        }
        mods.contains(self.modifiers)
    }
}

/// Human-readable key chord (e.g. `Ctrl+Shift+r`, `N`, `` ` ``).
#[allow(dead_code)] // used by dynamic help popup when that UI is re-enabled
pub fn format_spec(spec: &KeySpec) -> String {
    use KeyCode::*;
    if spec.code == BackTab {
        return "Shift+Tab".into();
    }
    let m = spec.modifiers;
    // Lone Shift+letter: show as `N` not `Shift+n`.
    if let Char(c) = spec.code {
        if c.is_ascii_lowercase() && m == KeyModifiers::SHIFT {
            return c.to_ascii_uppercase().to_string();
        }
    }

    let mut p = String::new();
    if m.contains(KeyModifiers::CONTROL) {
        p.push_str("Ctrl+");
    }
    if m.contains(KeyModifiers::SHIFT) {
        if matches!(spec.code, Char(_)) {
            p.push_str("Shift+");
        }
    }
    if m.contains(KeyModifiers::ALT) {
        p.push_str("Alt+");
    }
    if m.contains(KeyModifiers::SUPER) {
        p.push_str("Super+");
    }

    let key = match spec.code {
        Char(' ') => "Space".into(),
        Char(c) if c.is_ascii_alphabetic() => c.to_ascii_lowercase().to_string(),
        Char(c) => c.to_string(),
        Tab => "Tab".into(),
        Enter => "Enter".into(),
        Esc => "Esc".into(),
        Left => "←".into(),
        Right => "→".into(),
        Up => "↑".into(),
        Down => "↓".into(),
        Backspace => "Backspace".into(),
        PageUp => "PgUp".into(),
        PageDown => "PgDn".into(),
        _ => "?".into(),
    };
    format!("{p}{key}")
}

#[allow(dead_code)]
pub fn format_pair(a: &KeySpec, sep: &str, b: &KeySpec) -> String {
    format!("{}{}{}", format_spec(a), sep, format_spec(b))
}

#[allow(dead_code)]
pub fn format_optional(spec: &Option<KeySpec>) -> String {
    spec.as_ref().map_or("—".into(), format_spec)
}

// ── Keybinds ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Keybinds {
    pub scroll_up:          KeySpec,
    pub scroll_down:        KeySpec,
    pub column_left:        KeySpec,
    pub column_right:       KeySpec,
    pub play_pause:         KeySpec,
    pub next_track:         KeySpec,
    pub prev_track:         KeySpec,
    pub seek_forward:       KeySpec,
    pub seek_backward:      KeySpec,
    pub add_track:          KeySpec,
    pub add_all:                 KeySpec,
    pub add_all_replace_album:   Option<KeySpec>,
    pub add_all_replace_artist:  Option<KeySpec>,
    pub add_all_prepend:         Option<KeySpec>,
    pub shuffle:            KeySpec,
    pub unshuffle:          KeySpec,
    pub clear_queue:        KeySpec,
    pub search:             KeySpec,
    pub volume_up:          KeySpec,
    pub volume_down:        KeySpec,
    pub tab_switch:         KeySpec,
    /// Reverse tab cycle (Backtick by default)
    pub tab_switch_reverse: KeySpec,
    /// Jump to Home tab (default: '1')
    pub go_to_home:         KeySpec,
    /// Jump to Browser tab (default: '2')
    pub go_to_browser:      KeySpec,
    /// Jump to NowPlaying tab (default: '3')
    pub go_to_nowplaying:   KeySpec,
    pub quit:               KeySpec,
    /// Fuzzy library picker (`None` = disabled).
    pub library_fzf:        Option<KeySpec>,
    /// Force library index refresh (`None` = disabled).
    pub library_refresh:    Option<KeySpec>,
    pub toggle_help:          KeySpec,
    pub toggle_dynamic_theme: KeySpec,
    pub toggle_lyrics:        KeySpec,
    /// Primary visualizer toggle (default Shift+v). Bare `V` is still accepted in `map_key`.
    pub toggle_visualizer:    KeySpec,
    /// Browser: open/close playlist overlay (default Shift+p).
    pub playlist_overlay:     KeySpec,
    /// Browser: add focused track to playlist (`>`).
    pub browser_add_to_playlist: KeySpec,
    pub home_section_next:    KeySpec,
    pub home_section_prev:    KeySpec,
    pub home_refresh:         KeySpec,
}

impl Keybinds {
    pub fn from_section(sec: &KeybindsSection) -> Self {
        fn resolve(opt: Option<&str>, default: KeySpec) -> KeySpec {
            opt.and_then(parse_key).unwrap_or(default)
        }
        /// `None` in config → `default`; empty string → disabled (`None` output).
        fn resolve_opt(opt: Option<&str>, default: Option<KeySpec>) -> Option<KeySpec> {
            match opt {
                None => default,
                Some(s) if s.trim().is_empty() => None,
                Some(s) => parse_key(s),
            }
        }
        /// If the file still says `Ctrl+r` for index refresh while album replace is also `Ctrl+r`,
        /// treat refresh as unset so the default (`Ctrl+g`) wins. `Ctrl+space` is ignored too
        /// (many terminals never deliver it).
        fn library_refresh_config_str<'a>(
            sec: &'a KeybindsSection,
            add_all_replace_album: &Option<KeySpec>,
        ) -> Option<&'a str> {
            fn album_is_ctrl_r(spec: &Option<KeySpec>) -> bool {
                spec.as_ref().is_some_and(|k| {
                    k.code == KeyCode::Char('r') && k.modifiers == KeyModifiers::CONTROL
                })
            }
            match sec.library_refresh.as_deref().map(str::trim) {
                None => None,
                Some("") => Some(""),
                Some(s) if s.eq_ignore_ascii_case("ctrl+space") => None,
                Some(s) if s.eq_ignore_ascii_case("ctrl+r") && album_is_ctrl_r(add_all_replace_album) => {
                    None
                }
                Some(s) => Some(s),
            }
        }

        let add_all_replace_album = resolve_opt(
            sec.add_all_replace_album.as_deref(),
            Some(KeySpec {
                code: KeyCode::Char('r'),
                modifiers: KeyModifiers::CONTROL,
            }),
        );
        let add_all_replace_artist = resolve_opt(
            sec.add_all_replace_artist.as_deref(),
            Some(KeySpec {
                code: KeyCode::Char('r'),
                modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            }),
        );
        let add_all_prepend = resolve_opt(
            sec.add_all_prepend.as_deref(),
            Some(KeySpec {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            }),
        );
        let library_fzf = resolve_opt(
            sec.library_fzf.as_deref(),
            Some(KeySpec {
                code: KeyCode::Char('f'),
                modifiers: KeyModifiers::CONTROL,
            }),
        );
        let library_refresh = resolve_opt(
            library_refresh_config_str(sec, &add_all_replace_album),
            Some(KeySpec {
                code: KeyCode::Char('g'),
                modifiers: KeyModifiers::CONTROL,
            }),
        );

        Self {
            scroll_up:          resolve(sec.scroll_up.as_deref(),          KeySpec::new(KeyCode::Char('k'))),
            scroll_down:        resolve(sec.scroll_down.as_deref(),         KeySpec::new(KeyCode::Char('j'))),
            column_left:        resolve(sec.column_left.as_deref(),         KeySpec::new(KeyCode::Char('h'))),
            column_right:       resolve(sec.column_right.as_deref(),        KeySpec::new(KeyCode::Char('l'))),
            play_pause:         resolve(sec.play_pause.as_deref(),          KeySpec::new(KeyCode::Char('p'))),
            next_track:         resolve(sec.next_track.as_deref(),          KeySpec::new(KeyCode::Char('n'))),
            prev_track:         resolve(
                sec.prev_track.as_deref(),
                KeySpec {
                    code: KeyCode::Char('n'),
                    modifiers: KeyModifiers::SHIFT,
                },
            ),
            seek_forward:       resolve(sec.seek_forward.as_deref(),        KeySpec::new(KeyCode::Right)),
            seek_backward:      resolve(sec.seek_backward.as_deref(),       KeySpec::new(KeyCode::Left)),
            add_track:          resolve(sec.add_track.as_deref(),           KeySpec::new(KeyCode::Char('a'))),
            add_all:            resolve(
                sec.add_all.as_deref(),
                KeySpec {
                    code: KeyCode::Char('a'),
                    modifiers: KeyModifiers::SHIFT,
                },
            ),
            add_all_replace_album,
            add_all_replace_artist,
            add_all_prepend,
            shuffle:            resolve(sec.shuffle.as_deref(),             KeySpec::new(KeyCode::Char('x'))),
            unshuffle:          resolve(sec.unshuffle.as_deref(),           KeySpec::new(KeyCode::Char('z'))),
            clear_queue:        resolve(
                sec.clear_queue.as_deref(),
                KeySpec {
                    code: KeyCode::Char('d'),
                    modifiers: KeyModifiers::SHIFT,
                },
            ),
            search:             resolve(sec.search.as_deref(),              KeySpec::new(KeyCode::Char('/'))),
            volume_up:          resolve(sec.volume_up.as_deref(),           KeySpec::new(KeyCode::Char('+'))),
            volume_down:        resolve(sec.volume_down.as_deref(),         KeySpec::new(KeyCode::Char('-'))),
            tab_switch:         resolve(sec.tab_switch.as_deref(),          KeySpec::new(KeyCode::Tab)),
            tab_switch_reverse: resolve(sec.tab_switch_reverse.as_deref(),  KeySpec::new(KeyCode::Char('`'))),
            go_to_home:         resolve(sec.go_to_home.as_deref(),          KeySpec::new(KeyCode::Char('1'))),
            go_to_browser:      resolve(sec.go_to_browser.as_deref(),       KeySpec::new(KeyCode::Char('2'))),
            go_to_nowplaying:   resolve(sec.go_to_nowplaying.as_deref(),    KeySpec::new(KeyCode::Char('3'))),
            quit:               resolve(sec.quit.as_deref(),                KeySpec::new(KeyCode::Char('q'))),
            library_fzf,
            library_refresh,
            toggle_help: resolve(sec.toggle_help.as_deref(), KeySpec::new(KeyCode::Char('i'))),
            toggle_dynamic_theme: resolve(
                sec.toggle_dynamic_theme.as_deref(),
                KeySpec::new(KeyCode::Char('t')),
            ),
            toggle_lyrics: resolve(
                sec.toggle_lyrics.as_deref(),
                KeySpec {
                    code: KeyCode::Char('l'),
                    modifiers: KeyModifiers::SHIFT,
                },
            ),
            toggle_visualizer: resolve(
                sec.toggle_visualizer.as_deref(),
                KeySpec {
                    code: KeyCode::Char('v'),
                    modifiers: KeyModifiers::SHIFT,
                },
            ),
            playlist_overlay: resolve(
                sec.playlist_overlay.as_deref(),
                KeySpec {
                    code: KeyCode::Char('p'),
                    modifiers: KeyModifiers::SHIFT,
                },
            ),
            browser_add_to_playlist: resolve(
                sec.browser_add_to_playlist.as_deref(),
                KeySpec::new(KeyCode::Char('>')),
            ),
            home_section_next: resolve(
                sec.home_section_next.as_deref(),
                KeySpec {
                    code: KeyCode::Char('j'),
                    modifiers: KeyModifiers::SHIFT,
                },
            ),
            home_section_prev: resolve(
                sec.home_section_prev.as_deref(),
                KeySpec {
                    code: KeyCode::Char('k'),
                    modifiers: KeyModifiers::SHIFT,
                },
            ),
            home_refresh: resolve(sec.home_refresh.as_deref(), KeySpec::new(KeyCode::Char('r'))),
        }
    }
}

// ── Key string parser ─────────────────────────────────────────────────────────

/// Parse a user-supplied key string such as `"j"`, `"Shift+a"`, `"Tab"`, `"Left"`.
fn parse_key(s: &str) -> Option<KeySpec> {
    let s = s.trim();

    // "Shift+x" — lowercase letter + SHIFT (canonical; matches Ghostty / kitty protocols).
    if let Some(rest) = s.strip_prefix("Shift+").or_else(|| s.strip_prefix("shift+")) {
        if rest.len() == 1 {
            let c = rest.chars().next()?.to_ascii_lowercase();
            return Some(KeySpec {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::SHIFT,
            });
        }
        return None;
    }

    // Single printable character. Uppercase A–Z means Shift+letter (same as `Shift+a`).
    let chars: Vec<char> = s.chars().collect();
    if chars.len() == 1 {
        let c = chars[0];
        if c.is_ascii_uppercase() && c.is_ascii_alphabetic() {
            return Some(KeySpec {
                code: KeyCode::Char(c.to_ascii_lowercase()),
                modifiers: KeyModifiers::SHIFT,
            });
        }
        return Some(KeySpec::new(KeyCode::Char(c)));
    }

    // Ctrl+Shift+x
    if let Some(rest) = s.strip_prefix("Ctrl+Shift+").or_else(|| s.strip_prefix("ctrl+shift+")) {
        let mut it = rest.chars();
        if let Some(c) = it.next() {
            if it.next().is_none() {
                return Some(KeySpec {
                    code: KeyCode::Char(c.to_ascii_lowercase()),
                    modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                });
            }
        }
        return None;
    }

    // Ctrl+x (lowercase letter after prefix).
    if let Some(rest) = s.strip_prefix("Ctrl+").or_else(|| s.strip_prefix("ctrl+")) {
        if rest.eq_ignore_ascii_case("space") {
            return Some(KeySpec {
                code: KeyCode::Char(' '),
                modifiers: KeyModifiers::CONTROL,
            });
        }
        let mut it = rest.chars();
        if let Some(c) = it.next() {
            if it.next().is_none() {
                return Some(KeySpec {
                    code: KeyCode::Char(c.to_ascii_lowercase()),
                    modifiers: KeyModifiers::CONTROL,
                });
            }
        }
        return None;
    }

    // Named special keys.
    match s {
        "Tab"       => Some(KeySpec::new(KeyCode::Tab)),
        "Enter"     => Some(KeySpec::new(KeyCode::Enter)),
        "Esc"       => Some(KeySpec::new(KeyCode::Esc)),
        "Left"      => Some(KeySpec::new(KeyCode::Left)),
        "Right"     => Some(KeySpec::new(KeyCode::Right)),
        "Up"        => Some(KeySpec::new(KeyCode::Up)),
        "Down"      => Some(KeySpec::new(KeyCode::Down)),
        "Space"     => Some(KeySpec::new(KeyCode::Char(' '))),
        "Backspace" => Some(KeySpec::new(KeyCode::Backspace)),
        _           => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    use crate::config::KeybindsSection;

    #[test]
    fn single_uppercase_letter_in_config_is_shift_plus_lowercase() {
        let sec = KeybindsSection {
            prev_track: Some("N".into()),
            ..Default::default()
        };
        let kb = Keybinds::from_section(&sec);
        let p = &kb.prev_track;
        assert_eq!(p.code, KeyCode::Char('n'));
        assert_eq!(p.modifiers, KeyModifiers::SHIFT);
        assert!(p.matches(KeyCode::Char('n'), KeyModifiers::SHIFT));
        assert!(p.matches(KeyCode::Char('N'), KeyModifiers::empty()));
        assert!(!p.matches(KeyCode::Char('n'), KeyModifiers::empty()));
    }
}
