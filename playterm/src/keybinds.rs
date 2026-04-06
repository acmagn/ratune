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
    /// Uppercase chars match regardless of whether SHIFT is reported.
    pub fn matches(&self, code: KeyCode, mods: KeyModifiers) -> bool {
        if self.code != code {
            return false;
        }
        match self.code {
            // Uppercase letters: terminals may or may not report SHIFT separately.
            KeyCode::Char(c) if c.is_uppercase() => true,
            _ => mods.contains(self.modifiers),
        }
    }

    /// Human-readable label for display purposes.
    #[allow(dead_code)]
    pub fn display(&self) -> String {
        match self.code {
            KeyCode::Char(c) if c.is_uppercase() => {
                format!("S+{}", c.to_ascii_lowercase())
            }
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Tab      => "Tab".into(),
            KeyCode::Enter    => "Enter".into(),
            KeyCode::Esc      => "Esc".into(),
            KeyCode::Left     => "←".into(),
            KeyCode::Right    => "→".into(),
            KeyCode::Up       => "↑".into(),
            KeyCode::Down     => "↓".into(),
            KeyCode::Backspace => "Bksp".into(),
            _                 => "?".into(),
        }
    }
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
    pub add_all:            KeySpec,
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
        Self {
            scroll_up:          resolve(sec.scroll_up.as_deref(),          KeySpec::new(KeyCode::Char('k'))),
            scroll_down:        resolve(sec.scroll_down.as_deref(),         KeySpec::new(KeyCode::Char('j'))),
            column_left:        resolve(sec.column_left.as_deref(),         KeySpec::new(KeyCode::Char('h'))),
            column_right:       resolve(sec.column_right.as_deref(),        KeySpec::new(KeyCode::Char('l'))),
            play_pause:         resolve(sec.play_pause.as_deref(),          KeySpec::new(KeyCode::Char('p'))),
            next_track:         resolve(sec.next_track.as_deref(),          KeySpec::new(KeyCode::Char('n'))),
            prev_track:         resolve(sec.prev_track.as_deref(),          KeySpec::new(KeyCode::Char('N'))),
            seek_forward:       resolve(sec.seek_forward.as_deref(),        KeySpec::new(KeyCode::Right)),
            seek_backward:      resolve(sec.seek_backward.as_deref(),       KeySpec::new(KeyCode::Left)),
            add_track:          resolve(sec.add_track.as_deref(),           KeySpec::new(KeyCode::Char('a'))),
            add_all:            resolve(sec.add_all.as_deref(),             KeySpec::new(KeyCode::Char('A'))),
            shuffle:            resolve(sec.shuffle.as_deref(),             KeySpec::new(KeyCode::Char('x'))),
            unshuffle:          resolve(sec.unshuffle.as_deref(),           KeySpec::new(KeyCode::Char('z'))),
            clear_queue:        resolve(sec.clear_queue.as_deref(),         KeySpec::new(KeyCode::Char('D'))),
            search:             resolve(sec.search.as_deref(),              KeySpec::new(KeyCode::Char('/'))),
            volume_up:          resolve(sec.volume_up.as_deref(),           KeySpec::new(KeyCode::Char('+'))),
            volume_down:        resolve(sec.volume_down.as_deref(),         KeySpec::new(KeyCode::Char('-'))),
            tab_switch:         resolve(sec.tab_switch.as_deref(),          KeySpec::new(KeyCode::Tab)),
            tab_switch_reverse: resolve(sec.tab_switch_reverse.as_deref(),  KeySpec::new(KeyCode::Char('`'))),
            go_to_home:         resolve(sec.go_to_home.as_deref(),          KeySpec::new(KeyCode::Char('1'))),
            go_to_browser:      resolve(sec.go_to_browser.as_deref(),       KeySpec::new(KeyCode::Char('2'))),
            go_to_nowplaying:   resolve(sec.go_to_nowplaying.as_deref(),    KeySpec::new(KeyCode::Char('3'))),
            quit:               resolve(sec.quit.as_deref(),                KeySpec::new(KeyCode::Char('q'))),
            library_fzf: resolve_opt(
                sec.library_fzf.as_deref(),
                Some(KeySpec {
                    code: KeyCode::Char('f'),
                    modifiers: KeyModifiers::CONTROL,
                }),
            ),
            library_refresh: resolve_opt(
                sec.library_refresh.as_deref(),
                Some(KeySpec {
                    code: KeyCode::Char('r'),
                    modifiers: KeyModifiers::CONTROL,
                }),
            ),
        }
    }
}

// ── Key string parser ─────────────────────────────────────────────────────────

/// Parse a user-supplied key string such as `"j"`, `"Shift+a"`, `"Tab"`, `"Left"`.
fn parse_key(s: &str) -> Option<KeySpec> {
    let s = s.trim();

    // "Shift+x" — produce the uppercase char which covers both reporting styles.
    if let Some(rest) = s.strip_prefix("Shift+").or_else(|| s.strip_prefix("shift+")) {
        if rest.len() == 1 {
            let c = rest.chars().next()?.to_ascii_uppercase();
            return Some(KeySpec { code: KeyCode::Char(c), modifiers: KeyModifiers::SHIFT });
        }
        return None;
    }

    // Single printable character.
    let chars: Vec<char> = s.chars().collect();
    if chars.len() == 1 {
        return Some(KeySpec::new(KeyCode::Char(chars[0])));
    }

    // Ctrl+x (lowercase letter after prefix).
    if let Some(rest) = s.strip_prefix("Ctrl+").or_else(|| s.strip_prefix("ctrl+")) {
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
