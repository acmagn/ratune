/// Runtime theme — resolved ratatui `Color` values built from `ThemeSection`.
///
/// All fields default to the current hardcoded palette so the appearance is
/// identical when no `[theme]` section is present in config.toml.
///
/// Optional `[theme]` colour strings accept:
/// - `#rrggbb` or `rrggbb` (RGB),
/// - terminal indices: `idx:N`, `indexed:N`, `ansi:N`, `color:N`, or `i:N` for `N` in `0..=255`,
/// - `reset` / `inherit` / `default` / `unset` / `none` / `transparent` → do not paint a
///   background (terminal transparency / default bg).
use image::Rgba;
use ratatui::style::{Color, Style};
use ratatui::symbols::border;
use ratatui::widgets::BorderType;

use crate::config::{ThemeBorderSource, ThemeIconSection, ThemePreset, ThemeSection};

/// ASCII-friendly box borders (`+`, `-`, `|`) for fonts without box-drawing glyphs.
pub const ASCII_BORDER_SET: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Resolved UI glyphs from `[theme.icon]` (owned strings; defaults match prior hardcoding).
#[derive(Debug, Clone)]
pub struct ThemeIcons {
    /// Shown while a track is playing.
    pub playing: String,
    /// Shown while paused.
    pub paused: String,
    /// Shown when nothing is loaded.
    pub stopped: String,
    pub next_song: String,
    pub previous_song: String,
    pub mode_shuffle: String,
    pub mode_loop: String,
    /// Favorite / starred marker (without trailing space).
    pub favorite: String,
    /// Full tab separator including spaces (e.g. `" │ "`).
    pub tab_separator: String,
    pub online: String,
    pub offline: String,
    /// Radio live indicator glyph (without the ` LIVE` suffix).
    pub live: String,
}

impl Default for ThemeIcons {
    fn default() -> Self {
        Self {
            playing: "( ⏸ )".into(),
            paused: "( ▶ )".into(),
            stopped: "▶".into(),
            next_song: "⏭".into(),
            previous_song: "⏮".into(),
            mode_shuffle: "⇄".into(),
            mode_loop: "↻".into(),
            favorite: "★".into(),
            tab_separator: " │ ".into(),
            online: "●".into(),
            offline: "○".into(),
            live: "●".into(),
        }
    }
}

impl ThemeIcons {
    pub fn from_section(sec: &ThemeIconSection) -> Self {
        let d = Self::default();
        Self {
            playing: sec.playing.clone().unwrap_or(d.playing),
            paused: sec.paused.clone().unwrap_or(d.paused),
            stopped: sec.stopped.clone().unwrap_or(d.stopped),
            next_song: sec.next_song.clone().unwrap_or(d.next_song),
            previous_song: sec.previous_song.clone().unwrap_or(d.previous_song),
            mode_shuffle: sec.mode_shuffle.clone().unwrap_or(d.mode_shuffle),
            mode_loop: sec.mode_loop.clone().unwrap_or(d.mode_loop),
            favorite: sec.favorite.clone().unwrap_or(d.favorite),
            tab_separator: sec.tab_separator.clone().unwrap_or(d.tab_separator),
            online: sec.online.clone().unwrap_or(d.online),
            offline: sec.offline.clone().unwrap_or(d.offline),
            live: sec.live.clone().unwrap_or(d.live),
        }
    }

    /// `"★ "` when favorite is non-empty, else `""`.
    pub fn favorite_prefix(&self) -> String {
        if self.favorite.is_empty() {
            String::new()
        } else {
            format!("{} ", self.favorite)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub preset: ThemePreset,
    /// Orange accent: active borders, highlighted items, progress bar fill. (#ff8c00)
    pub accent: Color,
    /// General chrome background (popups, list fallbacks, selection inverse fg). (#1a1a1a)
    pub background: Color,
    /// Tab indicator bar background. Falls back to [`Self::background`] when unset in config.
    pub tab_bar: Color,
    /// Bottom status bar background. Falls back to [`Self::background`] when unset in config.
    pub status_bar: Color,
    /// Panel backgrounds (browser columns, queue block). (#161616)
    pub surface: Color,
    /// Primary text. (#d4d0c8)
    pub foreground: Color,
    /// Secondary / muted text. (#5a5858)
    pub dimmed: Color,
    /// Inactive pane borders. (#252525)
    pub border: Color,
    /// Active pane borders. (#3a3a3a)
    pub border_active: Color,
    /// Whether to use the dynamic accent extracted from album art.
    pub dynamic: bool,
    /// Transport / chrome glyphs from `[theme.icon]`.
    pub icons: ThemeIcons,
    /// Pane box-drawing set from `[theme.border_lines]`
    /// (legacy: flat `[theme].border_*` / `[theme.icon].border_*`).
    pub border_set: border::Set,
}

impl Theme {
    pub fn from_section(sec: &ThemeSection) -> Self {
        fn apply(opt: Option<&str>, base: Color) -> Color {
            opt.and_then(parse_theme_color).unwrap_or(base)
        }

        let preset = crate::config::theme_preset_from_section(sec);
        let mut theme = match preset {
            ThemePreset::Terminal => {
                let chrome = Color::Reset;
                Self {
                    preset,
                    // Use the terminal's palette / defaults. These indices follow the common ANSI
                    // mapping: 0..7 normal colors, 8..15 bright variants.
                    //
                    // - background/surface/tab_bar/status_bar: Reset (no painted bg)
                    // - foreground: Reset (inherit terminal default fg)
                    // - dimmed/border: bright black / "gray"
                    // - accent: blue-ish (4) by convention (matches ncmpcpp-ish defaults), but users
                    //   can tune their terminal theme to change what "4" means.
                    accent: Color::Indexed(4),
                    background: chrome,
                    tab_bar: chrome,
                    status_bar: chrome,
                    surface: Color::Reset,
                    foreground: Color::Reset,
                    dimmed: Color::Indexed(8),
                    border: Color::Indexed(8),
                    border_active: Color::Indexed(4),
                    dynamic: false,
                    icons: ThemeIcons::default(),
                    border_set: BorderType::Plain.to_border_set(),
                }
            }
            ThemePreset::Static => {
                let chrome = Color::Rgb(26, 26, 26);
                Self {
                    preset,
                    accent: Color::Rgb(255, 140, 0),
                    background: chrome,
                    tab_bar: chrome,
                    status_bar: chrome,
                    surface: Color::Rgb(22, 22, 22),
                    foreground: Color::Rgb(212, 208, 200),
                    dimmed: Color::Rgb(90, 88, 88),
                    border: Color::Rgb(37, 37, 37),
                    border_active: Color::Rgb(58, 58, 58),
                    dynamic: false,
                    icons: ThemeIcons::default(),
                    border_set: BorderType::Plain.to_border_set(),
                }
            }
            ThemePreset::Dynamic => {
                let chrome = Color::Rgb(26, 26, 26);
                Self {
                    preset,
                    accent: Color::Rgb(255, 140, 0),
                    background: chrome,
                    tab_bar: chrome,
                    status_bar: chrome,
                    surface: Color::Rgb(22, 22, 22),
                    foreground: Color::Rgb(212, 208, 200),
                    dimmed: Color::Rgb(90, 88, 88),
                    border: Color::Rgb(37, 37, 37),
                    border_active: Color::Rgb(58, 58, 58),
                    dynamic: true,
                    icons: ThemeIcons::default(),
                    border_set: BorderType::Plain.to_border_set(),
                }
            }
        };

        theme.icons = ThemeIcons::from_section(&sec.icon);
        theme.border_set = resolve_border_set(&sec.border_source());

        let chrome_default = theme.background;

        theme.accent = apply(sec.accent.as_deref(), theme.accent);
        theme.background = apply(sec.background.as_deref(), theme.background);
        theme.surface = apply(sec.surface.as_deref(), theme.surface);
        theme.foreground = apply(sec.foreground.as_deref(), theme.foreground);
        theme.dimmed = apply(sec.dimmed.as_deref(), theme.dimmed);
        theme.border = apply(sec.border.as_deref(), theme.border);
        theme.border_active = apply(sec.border_active.as_deref(), theme.border_active);

        // Bar colours: explicit `tab_bar` / `status_bar` win; else legacy `background` applies.
        let tab_bar_src = sec.tab_bar.as_deref().or(sec.background.as_deref());
        theme.tab_bar = apply(tab_bar_src, chrome_default);
        let status_bar_src = sec.status_bar.as_deref().or(sec.background.as_deref());
        theme.status_bar = apply(status_bar_src, chrome_default);

        theme
    }

    /// Return the accent colour to use for rendering: the dynamic extracted
    /// colour when `self.dynamic` is true and one is provided, else the
    /// static configured accent.
    pub fn effective_accent(&self, dynamic_accent: Option<Color>) -> Color {
        if self.dynamic {
            dynamic_accent.unwrap_or(self.accent)
        } else {
            self.accent
        }
    }
}

fn border_type_preset(name: &str) -> Option<border::Set> {
    match name.trim().to_ascii_lowercase().as_str() {
        "plain" | "normal" | "single" => Some(BorderType::Plain.to_border_set()),
        "rounded" | "round" => Some(BorderType::Rounded.to_border_set()),
        "double" => Some(BorderType::Double.to_border_set()),
        "thick" | "bold" => Some(BorderType::Thick.to_border_set()),
        "ascii" | "plus" => Some(ASCII_BORDER_SET),
        _ => None,
    }
}

fn leak_glyph(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn resolve_border_set(sec: &ThemeBorderSource<'_>) -> border::Set {
    let base = sec
        .border_type
        .and_then(border_type_preset)
        .unwrap_or_else(|| BorderType::Plain.to_border_set());

    let has_override = sec.top_left.is_some()
        || sec.top_right.is_some()
        || sec.bottom_left.is_some()
        || sec.bottom_right.is_some()
        || sec.vertical.is_some()
        || sec.horizontal.is_some();

    if !has_override {
        return base;
    }

    let vert = sec.vertical.map(leak_glyph).unwrap_or(base.vertical_left);
    let horiz = sec
        .horizontal
        .map(leak_glyph)
        .unwrap_or(base.horizontal_top);

    border::Set {
        top_left: sec.top_left.map(leak_glyph).unwrap_or(base.top_left),
        top_right: sec.top_right.map(leak_glyph).unwrap_or(base.top_right),
        bottom_left: sec.bottom_left.map(leak_glyph).unwrap_or(base.bottom_left),
        bottom_right: sec
            .bottom_right
            .map(leak_glyph)
            .unwrap_or(base.bottom_right),
        vertical_left: vert,
        vertical_right: sec.vertical.map(leak_glyph).unwrap_or(base.vertical_right),
        horizontal_top: horiz,
        horizontal_bottom: sec
            .horizontal
            .map(leak_glyph)
            .unwrap_or(base.horizontal_bottom),
    }
}

/// Build a [`Style`] with a background only when `c` is a real colour.
///
/// [`Color::Reset`] (from `reset`, `unset`, `transparent`, etc.) leaves the style without a
/// background so transparent terminals are not painted over.
pub fn style_with_bg(c: Color) -> Style {
    if c == Color::Reset {
        Style::default()
    } else {
        Style::default().bg(c)
    }
}

/// Parse a 6-digit hex colour string (with or without leading `#`).
/// Solid RGBA for `ratatui-image` padding (Sixel has no transparency — must match panel bg).
pub fn color_to_rgba(c: Color) -> Rgba<u8> {
    match c {
        Color::Rgb(r, g, b) => Rgba([r, g, b, 255]),
        // 16/256-colour terminals: approximate with dark grey (same default as `surface`).
        Color::Indexed(_) | Color::Reset => Rgba([22, 22, 22, 255]),
        // Named ANSI colours — pad with a neutral dark grey (theme is usually Rgb).
        _ => Rgba([22, 22, 22, 255]),
    }
}

/// Pad colour for album-art letterboxing: transparent when `surface` is unset.
pub fn surface_pad_rgba(c: Color) -> Rgba<u8> {
    if c == Color::Reset {
        Rgba([0, 0, 0, 0])
    } else {
        color_to_rgba(c)
    }
}

/// Parse a theme colour from config: hex RGB, terminal index (`idx:` / `ansi:` / …), or reset.
fn parse_theme_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "reset" | "inherit" | "default" | "unset" | "none" | "transparent" => {
            return Some(Color::Reset);
        }
        _ => {}
    }

    const INDEX_PREFIXES: &[&str] = &["indexed:", "idx:", "ansi:", "color:", "i:"];
    for p in INDEX_PREFIXES {
        if s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p) {
            let rest = s[p.len()..].trim();
            let n: u32 = rest.parse().ok()?;
            return (n <= 255).then_some(Color::Indexed(n as u8));
        }
    }

    parse_hex(s)
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_theme_color_hex() {
        assert_eq!(
            parse_theme_color("#76cce0"),
            Some(Color::Rgb(0x76, 0xcc, 0xe0))
        );
        assert_eq!(
            parse_theme_color("76cce0"),
            Some(Color::Rgb(0x76, 0xcc, 0xe0))
        );
    }

    #[test]
    fn parse_theme_color_indexed() {
        assert_eq!(parse_theme_color("idx:2"), Some(Color::Indexed(2)));
        assert_eq!(parse_theme_color("IDX: 14 "), Some(Color::Indexed(14)));
        assert_eq!(parse_theme_color("ansi:255"), Some(Color::Indexed(255)));
        assert_eq!(parse_theme_color("color:0"), Some(Color::Indexed(0)));
        assert_eq!(parse_theme_color("i:6"), Some(Color::Indexed(6)));
        assert_eq!(parse_theme_color("indexed:1"), Some(Color::Indexed(1)));
    }

    #[test]
    fn parse_theme_color_invalid_index() {
        assert_eq!(parse_theme_color("idx:256"), None);
        assert_eq!(parse_theme_color("idx:abc"), None);
    }

    #[test]
    fn terminal_preset_accepts_hex_override() {
        let sec = crate::config::ThemeSection {
            preset: Some("terminal".into()),
            accent: Some("#76cce0".into()),
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.preset, ThemePreset::Terminal);
        assert_eq!(t.accent, Color::Rgb(0x76, 0xcc, 0xe0));
        assert_eq!(t.background, Color::Reset);
    }

    #[test]
    fn static_preset_accepts_idx_override() {
        let sec = crate::config::ThemeSection {
            preset: Some("static".into()),
            accent: Some("idx:3".into()),
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.preset, ThemePreset::Static);
        assert_eq!(t.accent, Color::Indexed(3));
        assert_eq!(t.background, Color::Rgb(26, 26, 26));
    }

    #[test]
    fn parse_theme_color_unset_aliases() {
        for s in ["unset", "none", "transparent", "UNSET"] {
            assert_eq!(parse_theme_color(s), Some(Color::Reset), "{s}");
        }
    }

    #[test]
    fn legacy_background_applies_to_tab_and_status_bars() {
        let sec = crate::config::ThemeSection {
            background: Some("#000000".into()),
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.background, Color::Rgb(0, 0, 0));
        assert_eq!(t.tab_bar, Color::Rgb(0, 0, 0));
        assert_eq!(t.status_bar, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn tab_bar_and_status_bar_override_legacy_background() {
        let sec = crate::config::ThemeSection {
            background: Some("#000000".into()),
            tab_bar: Some("unset".into()),
            status_bar: Some("#111111".into()),
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.background, Color::Rgb(0, 0, 0));
        assert_eq!(t.tab_bar, Color::Reset);
        assert_eq!(t.status_bar, Color::Rgb(0x11, 0x11, 0x11));
    }

    #[test]
    fn theme_icons_from_section_override() {
        let sec = crate::config::ThemeSection {
            icon: crate::config::ThemeIconSection {
                playing: Some("||".into()),
                paused: Some("( > )".into()),
                stopped: Some(">".into()),
                next_song: Some(">>".into()),
                previous_song: Some("<<".into()),
                mode_shuffle: Some("><".into()),
                mode_loop: Some("o".into()),
                favorite: Some("*".into()),
                tab_separator: Some(" | ".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.icons.playing, "||");
        assert_eq!(t.icons.paused, "( > )");
        assert_eq!(t.icons.stopped, ">");
        assert_eq!(t.icons.next_song, ">>");
        assert_eq!(t.icons.previous_song, "<<");
        assert_eq!(t.icons.mode_shuffle, "><");
        assert_eq!(t.icons.mode_loop, "o");
        assert_eq!(t.icons.favorite, "*");
        assert_eq!(t.icons.tab_separator, " | ");
        assert_eq!(t.icons.favorite_prefix(), "* ");
    }

    #[test]
    fn theme_border_type_on_theme_section() {
        let sec = crate::config::ThemeSection {
            border_lines: crate::config::ThemeBorderLinesSection {
                style: Some("ascii".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.border_set, ASCII_BORDER_SET);
    }

    #[test]
    fn theme_border_type_legacy_flat_theme_fallback() {
        let sec = crate::config::ThemeSection {
            border_type: Some("ascii".into()),
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.border_set, ASCII_BORDER_SET);
    }

    #[test]
    fn theme_border_type_legacy_icon_fallback() {
        let sec = crate::config::ThemeSection {
            icon: crate::config::ThemeIconSection {
                border_type: Some("ascii".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.border_set, ASCII_BORDER_SET);
    }

    #[test]
    fn theme_border_lines_prefers_over_legacy() {
        let sec = crate::config::ThemeSection {
            border_lines: crate::config::ThemeBorderLinesSection {
                style: Some("ascii".into()),
                ..Default::default()
            },
            border_type: Some("double".into()),
            icon: crate::config::ThemeIconSection {
                border_type: Some("thick".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.border_set, ASCII_BORDER_SET);
    }

    #[test]
    fn style_with_bg_skips_reset() {
        assert_eq!(style_with_bg(Color::Reset), Style::default());
        assert_eq!(
            style_with_bg(Color::Rgb(1, 2, 3)),
            Style::default().bg(Color::Rgb(1, 2, 3))
        );
    }
}
