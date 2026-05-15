//! Query the running terminal for palette RGB (OSC 4) and default foreground (OSC 10)
//! so the spectrum visualizer can interpolate `gradient_theme` / `gradient_height` stops
//! without guessing indexed colours. When queries fail, the visualizer falls back to
//! ordered dithering between exact `Color` stops (no fake RGB).

use std::collections::{BTreeSet, HashMap};

use ratatui::style::Color;

use crate::config::Config;

/// RGB values read from the terminal (OSC 4 for indices, OSC 10 for default fg).
#[derive(Debug, Default, Clone)]
pub struct GradientRgbCache {
    pub indexed: HashMap<u8, (u8, u8, u8)>,
    pub default_fg: Option<(u8, u8, u8)>,
}

/// Populate a cache when a gradient visualizer mode may use non-`Rgb` [`Color`] values.
///
/// On Unix, opens `/dev/tty` and issues OSC sequences (before the alternate screen is fine).
/// Inside **tmux**, queries are wrapped with `\ePtmux;…\e\\` so they reach the outer terminal;
/// that requires tmux 3.2+ with `allow-passthrough on` (or `all`) in `~/.tmux.conf`. If the
/// outer terminal does not answer OSC readback, the visualizer falls back to ordered dithering.
/// On non-Unix, returns [`None`].
pub fn try_query_visualizer_gradient_cache(
    theme: &crate::theme::Theme,
    config: &Config,
    in_tmux: bool,
) -> Option<GradientRgbCache> {
    #[cfg(not(unix))]
    {
        let _ = (theme, config, in_tmux);
        return None;
    }

    #[cfg(unix)]
    {
        let mode = config.visualizer_color_mode.trim().to_ascii_lowercase();
        if !mode.starts_with("gradient") {
            return None;
        }

        let mut indices: BTreeSet<u8> = BTreeSet::new();
        let mut need_default_fg = false;

        match mode.as_str() {
            "gradient_theme" => {
                collect_color_refs(theme.dimmed, &mut indices, &mut need_default_fg);
                collect_color_refs(theme.foreground, &mut indices, &mut need_default_fg);
                collect_color_refs(theme.accent, &mut indices, &mut need_default_fg);
            }
            "gradient_height" => {
                let accent = theme.accent;
                for s in &config.visualizer_colors {
                    collect_color_refs(
                        parse_visualizer_color_token(s, accent),
                        &mut indices,
                        &mut need_default_fg,
                    );
                }
            }
            _ => return None,
        }

        if indices.is_empty() && !need_default_fg {
            return None;
        }

        let mut cache = GradientRgbCache::default();
        for &i in &indices {
            if let Some(rgb) = query_osc4_indexed_rgb(i, in_tmux) {
                cache.indexed.insert(i, rgb);
            }
        }
        if need_default_fg {
            cache.default_fg = query_osc10_default_fg(in_tmux);
        }
        if cache.indexed.is_empty() && cache.default_fg.is_none() {
            None
        } else {
            Some(cache)
        }
    }
}

fn parse_visualizer_color_token(s: &str, accent: Color) -> Color {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("accent") {
        return accent;
    }
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            ) {
                return Color::Rgb(r, g, b);
            }
        }
    }
    if let Ok(idx) = s.parse::<u8>() {
        return Color::Indexed(idx);
    }
    accent
}

fn collect_color_refs(c: Color, indices: &mut BTreeSet<u8>, need_default_fg: &mut bool) {
    match c {
        Color::Indexed(i) => {
            indices.insert(i);
        }
        Color::Reset => *need_default_fg = true,
        Color::Black => {
            indices.insert(0);
        }
        Color::Red => {
            indices.insert(1);
        }
        Color::Green => {
            indices.insert(2);
        }
        Color::Yellow => {
            indices.insert(3);
        }
        Color::Blue => {
            indices.insert(4);
        }
        Color::Magenta => {
            indices.insert(5);
        }
        Color::Cyan => {
            indices.insert(6);
        }
        Color::Gray => {
            indices.insert(7);
        }
        Color::DarkGray => {
            indices.insert(8);
        }
        Color::LightRed => {
            indices.insert(9);
        }
        Color::LightGreen => {
            indices.insert(10);
        }
        Color::LightYellow => {
            indices.insert(11);
        }
        Color::LightBlue => {
            indices.insert(12);
        }
        Color::LightMagenta => {
            indices.insert(13);
        }
        Color::LightCyan => {
            indices.insert(14);
        }
        Color::White => {
            indices.insert(15);
        }
        Color::Rgb(_, _, _) => {}
    }
}

/// Resolve a stop to sRGB for lerping. [`Color::Rgb`] always resolves; indexed / reset / named
/// resolve only when present in `cache` from a successful terminal query.
pub fn stop_to_rgb(c: Color, cache: Option<&GradientRgbCache>) -> Option<(u8, u8, u8)> {
    match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(i) => cache?.indexed.get(&i).copied(),
        Color::Reset => cache?.default_fg,
        Color::Black => cache?.indexed.get(&0).copied(),
        Color::Red => cache?.indexed.get(&1).copied(),
        Color::Green => cache?.indexed.get(&2).copied(),
        Color::Yellow => cache?.indexed.get(&3).copied(),
        Color::Blue => cache?.indexed.get(&4).copied(),
        Color::Magenta => cache?.indexed.get(&5).copied(),
        Color::Cyan => cache?.indexed.get(&6).copied(),
        Color::Gray => cache?.indexed.get(&7).copied(),
        Color::DarkGray => cache?.indexed.get(&8).copied(),
        Color::LightRed => cache?.indexed.get(&9).copied(),
        Color::LightGreen => cache?.indexed.get(&10).copied(),
        Color::LightYellow => cache?.indexed.get(&11).copied(),
        Color::LightBlue => cache?.indexed.get(&12).copied(),
        Color::LightMagenta => cache?.indexed.get(&13).copied(),
        Color::LightCyan => cache?.indexed.get(&14).copied(),
        Color::White => cache?.indexed.get(&15).copied(),
    }
}

// ── OSC query (Unix) ──────────────────────────────────────────────────────────

#[cfg(unix)]
/// Wrap a full escape sequence for tmux DCS passthrough (same doubling rules as `kitty_art::apc`).
fn tmux_passthrough_wrap(inner: &str) -> String {
    let mut doubled = String::with_capacity(inner.len() + 8);
    for ch in inner.chars() {
        if ch == '\x1b' {
            doubled.push('\x1b');
            doubled.push('\x1b');
        } else {
            doubled.push(ch);
        }
    }
    format!("\x1bPtmux;\x1b{doubled}\x1b\x1b\\\x1b\\")
}

#[cfg(unix)]
fn query_osc4_indexed_rgb(index: u8, in_tmux: bool) -> Option<(u8, u8, u8)> {
    // ESC ] 4 ; Ps ; ? BEL — xterm / many VTEs reply with OSC 4 ; Ps ; spec ST|BEL
    let inner = format!("\x1b]4;{};?\x07", index);
    let q = if in_tmux {
        tmux_passthrough_wrap(&inner)
    } else {
        inner
    };
    let reply = query_tty_raw(&q)?;
    parse_osc4_reply_rgb(&reply, index)
}

#[cfg(unix)]
fn query_osc10_default_fg(in_tmux: bool) -> Option<(u8, u8, u8)> {
    let inner = "\x1b]10;?\x07";
    let q = if in_tmux {
        tmux_passthrough_wrap(inner)
    } else {
        inner.to_string()
    };
    let reply = query_tty_raw(&q)?;
    parse_osc10_reply_rgb(&reply)
}

#[cfg(unix)]
fn query_tty_raw(query: &str) -> Option<String> {
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    if crossterm::terminal::enable_raw_mode().is_err() {
        return None;
    }
    let write_ok = tty.write_all(query.as_bytes()).is_ok() && tty.flush().is_ok();
    if !write_ok {
        let _ = crossterm::terminal::disable_raw_mode();
        return None;
    }

    let mut buf = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(60) {
        if tty.read(&mut byte).ok()? != 1 {
            continue;
        }
        buf.push(byte[0]);
        if buf.ends_with(&[0x07]) {
            break;
        }
        if buf.len() >= 2 && buf[buf.len() - 2] == 0x1b && buf[buf.len() - 1] == b'\\' {
            break;
        }
        if buf.len() >= 512 {
            break;
        }
    }
    let _ = crossterm::terminal::disable_raw_mode();
    String::from_utf8(buf).ok()
}

fn parse_rgb_triplet_after_label(s: &str) -> Option<(u8, u8, u8)> {
    let lower = s.to_ascii_lowercase();
    let rest = if let Some(i) = lower.find("rgb:") {
        &s[i + 4..]
    } else if let Some(i) = lower.find('#') {
        &s[i + 1..]
    } else {
        return None;
    };
    let end = rest
        .find('\x07')
        .or_else(|| rest.find('\x1b'))
        .unwrap_or(rest.len());
    let body = rest[..end].trim_end_matches('\\').trim();
    if body.starts_with('#') {
        let h = body.trim_start_matches('#');
        if h.len() == 6 {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            return Some((r, g, b));
        }
        return None;
    }
    let parts: Vec<&str> = body.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let comp = |p: &str| -> Option<u8> {
        let p = p.trim();
        if p.is_empty() {
            return None;
        }
        let v = u32::from_str_radix(p, 16).ok()?;
        Some(if p.len() <= 2 {
            v as u8
        } else {
            // 4-digit xterm components are 0..65535
            ((v * 255 + 32_767) / 65_535).min(255) as u8
        })
    };
    Some((comp(parts[0])?, comp(parts[1])?, comp(parts[2])?))
}

fn parse_osc4_reply_rgb(reply: &str, index: u8) -> Option<(u8, u8, u8)> {
    // Expect "...4;<idx>;rgb:rrrr/gggg/bbbb..." or "#rrggbb"
    let needle = format!("4;{};", index);
    if let Some(pos) = reply.find(&needle) {
        if let Some(rgb) = parse_rgb_triplet_after_label(&reply[pos..]) {
            return Some(rgb);
        }
    }
    parse_rgb_triplet_after_label(reply)
}

fn parse_osc10_reply_rgb(reply: &str) -> Option<(u8, u8, u8)> {
    if let Some(pos) = reply.find("10;") {
        if let Some(rgb) = parse_rgb_triplet_after_label(&reply[pos..]) {
            return Some(rgb);
        }
    }
    parse_rgb_triplet_after_label(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn tmux_wrap_doubles_esc_and_closes_dcs() {
        let w = tmux_passthrough_wrap("\x1b]4;8;?\x07");
        assert!(w.starts_with("\x1bPtmux;\x1b\x1b\x1b]4;8;?\x07"));
        assert!(w.ends_with("\x1b\x1b\\\x1b\\"));
    }

    #[test]
    fn parse_osc4_rgb_slash_form() {
        let s = "\x1b]4;8;rgb:e5e5/e5e5/e5e5\x1b\\";
        let rgb = parse_osc4_reply_rgb(s, 8).expect("rgb");
        assert_eq!(rgb, (229, 229, 229));
    }

    #[test]
    fn parse_osc10_rgb() {
        let s = "\x1b]10;rgb:aaaa/bbbb/cccc\x07";
        let rgb = parse_osc10_reply_rgb(s).expect("rgb");
        assert_eq!(rgb, (170, 187, 204));
    }
}
