use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use ratatui_image::picker::ProtocolType;
use ratatui_image::StatefulImage;

use crate::app::{App, HomeSection, HomeState, RecentAlbum};
use crate::config::{Config, HomePanel};
use crate::theme::{style_with_bg, Theme};
use crate::ui::kitty_art::{
    art_strip_layout, art_strip_slot_thumb_rect, art_strip_thumb_hit, KITTY_STRIP_MAX_SLOTS,
    STRIP_GAP_COLS,
};

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
            Style::default()
                .fg(theme.border_active)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(theme.dimmed),
            Style::default().fg(theme.border),
        )
    };
    Block::default()
        .style(style_with_bg(theme.surface))
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
        .constraints([Constraint::Length(top_h), Constraint::Length(bottom_h)])
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
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
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

/// Inner content rect of a home panel block (inside the border).
pub fn home_panel_inner(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

/// Map a click inside the Recently Played panel to an album index, if any.
pub fn home_recent_album_index_at(
    x: u16,
    y: u16,
    inner: Rect,
    show_art: bool,
    scroll_offset: usize,
    album_count: usize,
) -> Option<usize> {
    if album_count == 0
        || y < inner.y
        || y >= inner.y + inner.height
        || x < inner.x
        || x >= inner.x + inner.width
    {
        return None;
    }

    if !show_art {
        let row = (y - inner.y) as usize;
        let idx = scroll_offset + row;
        return (idx < album_count).then_some(idx);
    }

    let layout = art_strip_layout(inner.width, inner.height);
    let rel_x = x.saturating_sub(inner.x);
    let rel_y = y.saturating_sub(inner.y);
    let (row_in_grid, col_in_grid) = art_strip_thumb_hit(&layout, rel_x, rel_y)?;
    let slot = (row_in_grid as usize) * layout.per_row + (col_in_grid as usize);
    let max_slots = layout.total_visible.min(KITTY_STRIP_MAX_SLOTS);
    if slot >= max_slots {
        return None;
    }
    let idx = scroll_offset + slot;
    (idx < album_count).then_some(idx)
}

// ── Top-level render ──────────────────────────────────────────────────────────

pub fn render_home_tab(
    f: &mut Frame,
    area: Rect,
    app: &mut App,
    accent: Color,
    help_visible: bool,
) {
    let cfg = &app.config;
    let theme = app.theme.clone();
    let Some(layout) = compute_home_layout(area, cfg) else {
        return;
    };

    let use_strip_graphics = app.home_strip_graphics_wanted(help_visible);

    render_home_panel(
        f,
        layout.top,
        layout.top_panel,
        app,
        accent,
        use_strip_graphics,
        &theme,
    );

    if layout.bottom_h == 0 {
        return;
    }

    render_home_panel(
        f,
        layout.bottom_left,
        layout.bottom_left_panel,
        app,
        accent,
        use_strip_graphics,
        &theme,
    );
    render_home_panel(
        f,
        layout.bottom_right,
        layout.bottom_right_panel,
        app,
        accent,
        use_strip_graphics,
        &theme,
    );
}

fn render_home_panel(
    f: &mut Frame,
    area: Rect,
    panel: HomePanel,
    app: &mut App,
    accent: Color,
    use_strip_graphics: bool,
    theme: &Theme,
) {
    let cell_px = app.cell_px;
    match panel {
        HomePanel::RecentAlbums => {
            let is_active = app.home.active_section == HomeSection::RecentAlbums;
            let albums_block = titled_block(" Recently Played ", is_active, accent, theme);
            let albums_inner = albums_block.inner(area);
            app.home_recent_albums_inner = Some(albums_inner);
            f.render_widget(albums_block, area);

            if !app.config.home_recent_albums_show_art {
                render_recent_albums_list(
                    f,
                    albums_inner,
                    &app.home.recent_albums,
                    app.home.album_scroll_offset,
                    app.home.album_selected_index,
                    accent,
                    is_active,
                );
                return;
            }

            if use_strip_graphics {
                if app.kitty_apc_graphics_ready() || app.ratatui_uses_kitty_apc() {
                    // Kitty strip images are drawn after `terminal.draw` in main.rs.
                    render_art_strip_labels(f, albums_inner, &app.home, accent, cell_px, is_active);
                } else if app.ratatui_art_ready() {
                    render_art_strip_ratatui(f, albums_inner, app, is_active);
                    render_art_strip_labels(f, albums_inner, &app.home, accent, cell_px, is_active);
                }
            } else {
                render_art_strip_text_fallback(
                    f,
                    albums_inner,
                    &app.home.recent_albums,
                    app.home.album_selected_index,
                    accent,
                    is_active,
                );
            }
        }
        HomePanel::RecentTracks => {
            render_recent_tracks_block(f, area, &app.home, accent, theme);
        }
        HomePanel::Rediscover => {
            render_rediscover_block(f, area, &app.home, accent, theme);
        }
    }
}

fn render_recent_albums_list(
    f: &mut Frame,
    area: Rect,
    albums: &[RecentAlbum],
    scroll_offset: usize,
    selected_index: usize,
    accent: Color,
    is_active: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    if albums.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  No album history yet",
                Style::default().fg(Color::DarkGray),
            ))),
            Rect { height: 1, ..area },
        );
        return;
    }

    // Recent-Tracks-like list, with the same scroll model as the strip (album_scroll_offset).
    let artist_w = ((area.width as usize).saturating_sub(8) * 35 / 100).max(10);
    let album_w = (area.width as usize).saturating_sub(8 + artist_w).max(10);

    let max_items = (area.height as usize).min(albums.len().saturating_sub(scroll_offset));
    let mut lines: Vec<Line> = Vec::new();
    for row in 0..max_items {
        let idx = scroll_offset + row;
        let a = &albums[idx];
        let text = format!(
            " {:>2}. {:<artist_w$} {:<album_w$}",
            idx + 1,
            truncate(&a.artist_name, artist_w),
            truncate(&a.album_name, album_w),
            artist_w = artist_w,
            album_w = album_w,
        );
        let selected = is_active && idx == selected_index;
        let style = if selected {
            Style::default().bg(accent).fg(Color::Black)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(text, style)));
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn render_art_strip_ratatui(f: &mut Frame, albums_inner: Rect, app: &mut App, _is_active: bool) {
    if let Some(p) = app.art_picker.as_mut() {
        p.set_background_color(crate::theme::surface_pad_rgba(app.theme.surface));
    }
    let Some(picker) = app.art_picker.as_ref() else {
        return;
    };
    if albums_inner.height < 3 {
        return;
    }
    let layout = art_strip_layout(albums_inner.width, albums_inner.height);
    let thumb_cols = layout.thumb_cols;
    let thumb_rows = layout.thumb_rows;
    let visible_count = layout.total_visible.min(KITTY_STRIP_MAX_SLOTS);

    let is_sixel = matches!(picker.protocol_type(), ProtocolType::Sixel);
    let img_resize = app.ratatui_stateful_resize_strip();
    const MAX_SIXEL_STRIP_BUILDS_PER_FRAME: usize = 3;
    let mut sixel_builds_this_frame = 0usize;
    let mut keep = HashSet::new();
    let fs = picker.font_size();

    for i in 0..visible_count {
        let album_index = app.home.album_scroll_offset + i;
        if album_index >= app.home.recent_albums.len() {
            break;
        }
        let album_id = app.home.recent_albums[album_index].album_id.clone();
        keep.insert(album_id.clone());

        let thumb_rect = art_strip_slot_thumb_rect(&layout, albums_inner, i);

        if let Some(bytes) = app.home_art_cache.get(&album_id) {
            let slot = (thumb_cols, thumb_rows);
            if app.home_strip_last_cells.get(&album_id).copied() != Some(slot) {
                app.home_strip_art.remove(&album_id);
            }
            let Ok(img) = image::load_from_memory(bytes) else {
                continue;
            };
            let art_rect = crate::ui::art_prepare::contain_fit_rect_in_cells(&img, thumb_rect, fs);
            if !app.home_strip_art.contains_key(&album_id) {
                if is_sixel && sixel_builds_this_frame >= MAX_SIXEL_STRIP_BUILDS_PER_FRAME {
                    continue;
                }
                let prepared = crate::ui::art_prepare::prepare_art_image_for_rect_contain_fit(
                    img, art_rect, fs,
                );
                let proto = picker.new_resize_protocol(prepared);
                app.home_strip_art.insert(album_id.clone(), proto);
                app.home_strip_last_cells.insert(album_id.clone(), slot);
                if is_sixel {
                    sixel_builds_this_frame += 1;
                }
            }
            if let Some(state) = app.home_strip_art.get_mut(&album_id) {
                let w = StatefulImage::default().resize(img_resize.clone());
                f.render_stateful_widget(w, art_rect, state);
            }
        }
    }

    app.home_strip_art.retain(|k, _| keep.contains(k));
    app.home_strip_last_cells.retain(|k, _| keep.contains(k));
}

// ── Art strip label rows (Kitty path) ─────────────────────────────────────────

/// Render album + artist text **under each row of thumbnails** (centered strip, per-column cells).
fn render_art_strip_labels(
    f: &mut Frame,
    inner: Rect,
    home: &HomeState,
    accent: Color,
    _cell_px: Option<(u16, u16)>,
    is_active: bool,
) {
    if inner.height < 3 {
        return;
    }

    let layout = art_strip_layout(inner.width, inner.height);
    let visible_count = layout.total_visible.min(KITTY_STRIP_MAX_SLOTS);
    let thumb_cols = layout.thumb_cols;

    for row_in_grid in 0u16..layout.grid_rows {
        let name_row_y = inner
            .y
            .saturating_add(layout.album_label_top_dy(row_in_grid));
        let artist_row_y = name_row_y + 1;
        if name_row_y >= inner.y + inner.height {
            break;
        }

        for col_in_grid in 0..layout.per_row {
            let slot = row_in_grid as usize * layout.per_row + col_in_grid;
            if slot >= visible_count {
                break;
            }
            let album_index = home.album_scroll_offset + slot;
            if album_index >= home.recent_albums.len() {
                break;
            }
            let album = &home.recent_albums[album_index];
            let is_selected = is_active && album_index == home.album_selected_index;

            let label_width = thumb_cols as usize;
            let name_label = pad_or_truncate(&album.album_name, label_width);
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

            let col_x = inner
                .x
                .saturating_add(layout.pad_x)
                .saturating_add((col_in_grid as u16) * (thumb_cols + STRIP_GAP_COLS));

            f.render_widget(
                Paragraph::new(Line::from(Span::styled(name_label, name_style))),
                Rect {
                    x: col_x,
                    y: name_row_y,
                    width: thumb_cols,
                    height: 1,
                },
            );
            if artist_row_y < inner.y + inner.height {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(artist_label, artist_style))),
                    Rect {
                        x: col_x,
                        y: artist_row_y,
                        width: thumb_cols,
                        height: 1,
                    },
                );
            }
        }
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
        f.render_widget(Paragraph::new(hint), Rect { height: 1, ..area });
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
                Rect {
                    y: area.y + 1,
                    height: 1,
                    ..area
                },
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
            Rect {
                y: area.y + area.height.saturating_sub(1),
                height: 1,
                ..area
            },
        );
    }
}

// ── Section block renderers ───────────────────────────────────────────────────

fn render_recent_tracks_block(
    f: &mut Frame,
    area: Rect,
    home: &HomeState,
    accent: Color,
    theme: &Theme,
) {
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

fn render_rediscover_block(
    f: &mut Frame,
    area: Rect,
    home: &HomeState,
    accent: Color,
    theme: &Theme,
) {
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
        let max_items = (inner.height as usize)
            .saturating_sub(1)
            .min(home.rediscover.len());
        for (i, (_, name)) in home.rediscover.iter().enumerate().take(max_items) {
            let text = format!(
                " {:>2}. {}",
                i + 1,
                truncate(name, inner.width as usize - 6)
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
    if max == 0 {
        return String::new();
    }
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
    if width == 0 {
        return String::new();
    }
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
