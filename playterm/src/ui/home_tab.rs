use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{HomeSection, HomeState, RecentAlbum};
use crate::config::{Config, HomePanel};
use crate::theme::Theme;
use crate::ui::kitty_art::{art_strip_thumbnail_size, visible_thumbnail_count};

// ── Relative time formatting ──────────────────────────────────────────────────

fn relative_time(played_at: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(played_at);
    let secs = (now - played_at).max(0) as u64;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86400 {
        format!("{} hr ago", secs / 3600)
    } else {
        format!("{} days ago", secs / 86400)
    }
}

// ── Block with optional accent-coloured title and themed borders ──────────────

fn titled_block<'a>(title: &'a str, is_active: bool, accent: Color, theme: &Theme) -> Block<'a> {
    let (title_style, border_style) = if is_active {
        (
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
            Style::default().fg(theme.border_active).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(theme.dimmed),
            Style::default().fg(theme.border),
        )
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style)
        .title(Span::styled(title, title_style))
}

// ── Layout (shared with mouse hit-testing in main.rs) ─────────────────────────

/// Resolved rectangles for the three Home panels. `bottom_*` are only meaningful when
/// `bottom_h > 0` (see [`compute_home_layout`]).
pub struct HomeLayout {
    pub top: Rect,
    pub bottom_left: Rect,
    pub bottom_right: Rect,
    pub top_panel: HomePanel,
    pub bottom_left_panel: HomePanel,
    pub bottom_right_panel: HomePanel,
    pub bottom_h: u16,
}

#[allow(dead_code)]
pub fn home_panel_to_section(panel: HomePanel) -> HomeSection {
    match panel {
        HomePanel::RecentAlbums => HomeSection::RecentAlbums,
        HomePanel::RecentTracks => HomeSection::RecentTracks,
        HomePanel::Rediscover => HomeSection::Rediscover,
    }
}

/// Split the Home content area into a top band and two bottom columns, following `[ui.hometab].layout`.
pub fn compute_home_layout(area: Rect, cfg: &Config) -> Option<HomeLayout> {
    if area.height == 0 {
        return None;
    }
    let top_pct = cfg.home_top_height_percent as u32;
    let top_h = ((area.height as u32 * top_pct / 100).max(3) as u16).min(area.height);
    let bottom_h = area.height.saturating_sub(top_h);

    let top_level = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_h),
            Constraint::Length(bottom_h),
        ])
        .split(area);

    let top_area = top_level[0];
    let bottom_area = top_level[1];
    let panels = cfg.home_panels;
    let top_panel = panels[0];
    let bottom_left_panel = panels[1];
    let bottom_right_panel = panels[2];

    if bottom_h == 0 {
        return Some(HomeLayout {
            top: top_area,
            bottom_left: Rect {
                x: area.x,
                y: area.y,
                width: 0,
                height: 0,
            },
            bottom_right: Rect {
                x: area.x,
                y: area.y,
                width: 0,
                height: 0,
            },
            top_panel,
            bottom_left_panel,
            bottom_right_panel,
            bottom_h: 0,
        });
    }

    let bottom_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(bottom_area);

    Some(HomeLayout {
        top: top_area,
        bottom_left: bottom_cols[0],
        bottom_right: bottom_cols[1],
        top_panel,
        bottom_left_panel,
        bottom_right_panel,
        bottom_h,
    })
}

// ── Top-level render ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn render_home_tab(
    f: &mut Frame,
    area: Rect,
    home: &HomeState,
    cfg: &Config,
    accent: Color,
    kitty_supported: bool,
    help_visible: bool,
    cell_px: Option<(u16, u16)>,
    theme: &Theme,
) {
    let Some(layout) = compute_home_layout(area, cfg) else {
        return;
    };

    let show_album_art = cfg.home_recent_albums_show_art;
    let use_kitty_art = kitty_supported && !help_visible && show_album_art;

    render_home_panel(
        f,
        layout.top,
        layout.top_panel,
        home,
        accent,
        use_kitty_art,
        cell_px,
        theme,
    );

    if layout.bottom_h == 0 {
        return;
    }

    render_home_panel(
        f,
        layout.bottom_left,
        layout.bottom_left_panel,
        home,
        accent,
        use_kitty_art,
        cell_px,
        theme,
    );
    render_home_panel(
        f,
        layout.bottom_right,
        layout.bottom_right_panel,
        home,
        accent,
        use_kitty_art,
        cell_px,
        theme,
    );
}

fn render_home_panel(
    f: &mut Frame,
    area: Rect,
    panel: HomePanel,
    home: &HomeState,
    accent: Color,
    use_kitty_art: bool,
    cell_px: Option<(u16, u16)>,
    theme: &Theme,
) {
    match panel {
        HomePanel::RecentAlbums => {
            let is_active = home.active_section == HomeSection::RecentAlbums;
            let albums_block = titled_block(" Recently Played ", is_active, accent, theme);
            let albums_inner = albums_block.inner(area);
            f.render_widget(albums_block, area);

            if use_kitty_art {
                // NOTE: render_art_strip (heavy) is driven from main.rs via `home_art_needs_redraw`.
                render_art_strip_labels(f, albums_inner, home, accent, cell_px, is_active);
            } else {
                render_art_strip_text_fallback(
                    f,
                    albums_inner,
                    &home.recent_albums,
                    home.album_selected_index,
                    accent,
                    is_active,
                );
            }
        }
        HomePanel::RecentTracks => {
            render_recent_tracks_block(f, area, home, accent, theme);
        }
        HomePanel::Rediscover => {
            render_rediscover_block(f, area, home, accent, theme);
        }
    }
}

// ── Art strip label rows (Kitty path) ─────────────────────────────────────────

/// Render album name + artist name text rows below the Kitty thumbnails,
/// inside the inner area of the Recently Played block.
fn render_art_strip_labels(
    f: &mut Frame,
    inner: Rect,
    home: &HomeState,
    accent: Color,
    cell_px: Option<(u16, u16)>,
    is_active: bool,
) {
    if inner.height < 3 {
        return;
    }

    let thumb_area_h = inner.height.saturating_sub(2).max(1);
    let (thumb_cols, _) = art_strip_thumbnail_size(cell_px, thumb_area_h);
    let visible_count = visible_thumbnail_count(inner.width, thumb_cols, 1);

    // Album name row: 1 row below the thumbnail strip.
    let name_row_y = inner.y + thumb_area_h;
    // Artist name row: 1 row below the album name row.
    let artist_row_y = name_row_y + 1;

    if name_row_y >= inner.y + inner.height {
        return;
    }

    let mut name_spans: Vec<Span> = Vec::new();
    let mut artist_spans: Vec<Span> = Vec::new();

    for i in 0..visible_count {
        let album_index = home.album_scroll_offset + i;
        if album_index >= home.recent_albums.len() {
            break;
        }
        let album = &home.recent_albums[album_index];
        let is_selected = is_active && album_index == home.album_selected_index;

        // Each label cell is `thumb_cols` wide (+ 1 gap, except last).
        let label_width = thumb_cols as usize;
        let name_label  = pad_or_truncate(&album.album_name, label_width);
        let artist_label = pad_or_truncate(&album.artist_name, label_width);

        let (name_style, artist_style) = if is_selected {
            (
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
                Style::default().fg(accent),
            )
        } else {
            (
                Style::default().fg(Color::Gray),
                Style::default().fg(Color::DarkGray),
            )
        };

        name_spans.push(Span::styled(name_label, name_style));
        name_spans.push(Span::raw(" ")); // gap
        artist_spans.push(Span::styled(artist_label, artist_style));
        artist_spans.push(Span::raw(" ")); // gap
    }

    f.render_widget(
        Paragraph::new(Line::from(name_spans)),
        Rect {
            x: inner.x,
            y: name_row_y,
            width: inner.width,
            height: 1,
        },
    );

    if artist_row_y < inner.y + inner.height {
        f.render_widget(
            Paragraph::new(Line::from(artist_spans)),
            Rect {
                x: inner.x,
                y: artist_row_y,
                width: inner.width,
                height: 1,
            },
        );
    }
}

// ── Art strip helpers ─────────────────────────────────────────────────────────

/// Text fallback for the art strip (non-Kitty terminals).
/// Renders a horizontal list of album names, with the selected one highlighted.
pub fn render_art_strip_text_fallback(
    f: &mut Frame,
    area: Rect,
    albums: &[RecentAlbum],
    selected_index: usize,
    accent: Color,
    is_active: bool,
) {
    if area.height == 0 {
        return;
    }

    if albums.is_empty() {
        let hint = Line::from(Span::styled(
            "  No album history yet",
            Style::default().fg(Color::DarkGray),
        ));
        f.render_widget(
            Paragraph::new(hint),
            Rect { height: 1, ..area },
        );
        return;
    }

    // Row 0: horizontal album list — each album name truncated to fit.
    let visible = (area.width as usize / 16).max(1);
    let mut spans: Vec<Span> = Vec::new();
    for (i, album) in albums.iter().enumerate().take(visible) {
        let label = format!(" {} ", truncate(&album.album_name, 14));
        let selected = is_active && i == selected_index;
        let style = if selected {
            Style::default().bg(accent).fg(Color::Black)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(label, style));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { height: 1, ..area },
    );

    // Row 1: show selected album info.
    if area.height > 1 {
        if let Some(album) = albums.get(selected_index) {
            let info = format!("  {} — {}", album.album_name, album.artist_name);
            f.render_widget(
                Paragraph::new(Line::from(Span::raw(info))),
                Rect { y: area.y + 1, height: 1, ..area },
            );
        }
    }

    // Remaining rows: key hint.
    if area.height > 2 && is_active {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  h/l navigate  Enter play  a add to queue",
                Style::default().fg(Color::DarkGray),
            ))),
            Rect { y: area.y + area.height.saturating_sub(1), height: 1, ..area },
        );
    }
}

// ── Section block renderers ───────────────────────────────────────────────────

fn render_recent_tracks_block(f: &mut Frame, area: Rect, home: &HomeState, accent: Color, theme: &Theme) {
    let is_active = home.active_section == HomeSection::RecentTracks;
    let block = titled_block(" Recent Tracks ", is_active, accent, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    if home.recent_tracks.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No play history yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let max_items = (inner.height as usize).min(home.recent_tracks.len());
        for (i, record) in home.recent_tracks.iter().enumerate().take(max_items) {
            let rel = relative_time(record.played_at);
            // Width budget: track ~40%, artist ~30%, time fills rest.
            let track_w = ((inner.width as usize).saturating_sub(8) * 40 / 100).max(10);
            let artist_w = ((inner.width as usize).saturating_sub(8) * 30 / 100).max(8);
            let text = format!(
                " {:>2}. {:<track_w$} {:<artist_w$} {}",
                i + 1,
                truncate(&record.track_name, track_w),
                truncate(&record.artist_name, artist_w),
                rel,
                track_w = track_w,
                artist_w = artist_w,
            );
            let selected = is_active && home.selected_index == i;
            let style = if selected {
                Style::default().bg(accent).fg(Color::Black)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn render_rediscover_block(f: &mut Frame, area: Rect, home: &HomeState, accent: Color, theme: &Theme) {
    let is_active = home.active_section == HomeSection::Rediscover;
    let block = titled_block(" Rediscover ", is_active, accent, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    if home.rediscover.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Listen to more music to unlock suggestions",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let max_items = (inner.height as usize).saturating_sub(1).min(home.rediscover.len());
        for (i, (_, name)) in home.rediscover.iter().enumerate().take(max_items) {
            let text = format!(" {:>2}. {}", i + 1, truncate(name, inner.width as usize - 6));
            let selected = is_active && home.selected_index == i;
            let style = if selected {
                Style::default().bg(accent).fg(Color::Black)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
    }

    // Re-roll hint on the last row.
    if inner.height > 0 {
        // Pad with empty lines to push the hint to the bottom.
        while lines.len() < inner.height.saturating_sub(1) as usize {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "  Press r to re-roll",
            Style::default().fg(Color::DarkGray),
        )));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Truncate `s` to at most `max` characters, adding `…` if truncated.
fn truncate(s: &str, max: usize) -> String {
    if max == 0 { return String::new(); }
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}'); // …
        out
    }
}

/// Pad `s` to exactly `width` chars, or truncate with `…` if longer.
fn pad_or_truncate(s: &str, width: usize) -> String {
    if width == 0 { return String::new(); }
    let count = s.chars().count();
    if count == width {
        s.to_string()
    } else if count < width {
        let mut out = s.to_string();
        for _ in 0..(width - count) {
            out.push(' ');
        }
        out
    } else {
        truncate(s, width)
    }
}
