//! Playlist browser overlay.
//!
//! Renders a two-column panel covering the bottom 40% of the browser content
//! area.  Left column shows the playlist list; right column shows the tracks
//! of the currently selected playlist.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style}; // Modifier used by highlight_style
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::PlaylistPicker;
use crate::state::{
    ConfirmAction, LoadingState, PlaylistFocus, PlaylistInputMode, PlaylistOverlay,
};
use crate::theme::style_with_bg;
use crate::theme::Theme;

// ── Duration helpers ──────────────────────────────────────────────────────────

/// Format a total-seconds value as "Xh Ym" when ≥ 1 hour, else "Xm Ys".
fn fmt_duration_hm(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}h {}m", h, m)
    } else {
        format!("{}m {}s", m, s)
    }
}

/// Format a per-track duration (seconds) as "m:ss".
fn fmt_duration_ms(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{}:{:02}", m, s)
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Render the playlist overlay on top of the browser content `area`.
///
/// Does nothing when `overlay.visible` is `false`.
pub fn render_playlist_overlay(
    frame: &mut Frame,
    area: Rect,
    overlay: &PlaylistOverlay,
    accent: Color,
    theme: &Theme,
) {
    if !overlay.visible {
        return;
    }

    // ── Overlay occupies the bottom 40% of the browser content area ──────────

    let split =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(area);

    let overlay_area = split[1];

    // Clear all cells in the overlay region before drawing so browser content
    // beneath does not bleed through the Block widgets.
    frame.render_widget(Clear, overlay_area);

    // ── Horizontal split: left 35% (playlists), right 65% (tracks) ───────────

    let cols = Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(overlay_area);

    let left_active = matches!(overlay.focus, PlaylistFocus::List);
    let right_active = !left_active;

    render_playlist_list(frame, cols[0], overlay, accent, theme, left_active);
    render_track_list(frame, cols[1], overlay, accent, theme, right_active);

    // Confirm dialog renders on top of both columns when active.
    if let PlaylistInputMode::Confirming { action } = &overlay.input_mode {
        let msg = match action {
            ConfirmAction::DeletePlaylist { name, .. } => {
                format!(" Delete \"{}\"? [y/n] ", name)
            }
        };
        render_confirm_dialog(frame, overlay_area, &msg, accent, theme);
    }
}

// ── Left column ───────────────────────────────────────────────────────────────

fn render_playlist_list(
    frame: &mut Frame,
    area: Rect,
    overlay: &PlaylistOverlay,
    accent: Color,
    theme: &Theme,
    is_active: bool,
) {
    // Reserve 3 rows for the text-input box when creating/renaming.
    let (list_area, input_area) = match &overlay.input_mode {
        PlaylistInputMode::Creating { .. } | PlaylistInputMode::Renaming { .. }
            if area.height > 3 =>
        {
            let split = Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).split(area);
            (split[0], Some(split[1]))
        }
        _ => (area, None),
    };

    let border_color = if is_active { accent } else { theme.border };
    let title_color = if is_active { accent } else { theme.dimmed };
    let list_border = if is_active {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };

    let block = Block::default()
        .title(" Playlists ")
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(list_border)
        .style(style_with_bg(theme.background));

    let (items, sel) = match &overlay.playlists {
        LoadingState::NotLoaded => (vec![], None),
        LoadingState::Loading => (
            vec![ListItem::new("Loading…").style(Style::default().fg(theme.dimmed))],
            None,
        ),
        LoadingState::Error(e) => (
            vec![ListItem::new(format!("Error: {e}")).style(Style::default().fg(accent))],
            None,
        ),
        LoadingState::Loaded(playlists) => {
            if playlists.is_empty() {
                (
                    vec![ListItem::new("No playlists").style(Style::default().fg(theme.dimmed))],
                    None,
                )
            } else {
                let items = playlists
                    .iter()
                    .map(|p| {
                        let count = p.song_count.unwrap_or(0);
                        let label = match p.duration {
                            Some(d) => {
                                format!("{}  ({} tracks · {})", p.name, count, fmt_duration_hm(d))
                            }
                            None => format!("{}  ({} tracks)", p.name, count),
                        };
                        ListItem::new(label).style(Style::default().fg(theme.foreground))
                    })
                    .collect();
                let sel = Some(overlay.selected_playlist_index.min(playlists.len() - 1));
                (items, sel)
            }
        }
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(accent)
                .fg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ")
        .style(style_with_bg(theme.background));

    let mut state = ListState::default();
    state.select(sel);
    frame.render_stateful_widget(list, list_area, &mut state);

    // Text-input box (create / rename).
    if let Some(input_rect) = input_area {
        let (label, buffer) = match &overlay.input_mode {
            PlaylistInputMode::Creating { buffer } => (" New playlist name ", buffer.as_str()),
            PlaylistInputMode::Renaming { buffer, .. } => (" Rename playlist ", buffer.as_str()),
            _ => unreachable!(),
        };
        let input_block = Block::default()
            .title(label)
            .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(accent))
            .style(style_with_bg(theme.background));
        let input_line = Line::from(vec![
            Span::styled(buffer, Style::default().fg(theme.foreground)),
            Span::styled("_", Style::default().fg(accent)),
        ]);
        frame.render_widget(Paragraph::new(input_line).block(input_block), input_rect);
    }
}

// ── Right column ──────────────────────────────────────────────────────────────

fn render_track_list(
    frame: &mut Frame,
    area: Rect,
    overlay: &PlaylistOverlay,
    accent: Color,
    theme: &Theme,
    is_active: bool,
) {
    let border_color = if is_active { accent } else { theme.border };
    let title_color = if is_active { accent } else { theme.dimmed };
    let list_border = if is_active {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };

    // Show the selected playlist's name, or a fallback prompt.
    let title_text = match &overlay.playlists {
        LoadingState::Loaded(playlists) if !playlists.is_empty() => playlists
            .get(overlay.selected_playlist_index)
            .map(|p| format!(" {} ", p.name))
            .unwrap_or_else(|| " Select a playlist ".to_string()),
        _ => " Select a playlist ".to_string(),
    };

    let block = Block::default()
        .title(title_text)
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(list_border)
        .style(style_with_bg(theme.background));

    let (items, sel) = match &overlay.tracks {
        LoadingState::NotLoaded => (
            vec![ListItem::new("Select a playlist").style(Style::default().fg(theme.dimmed))],
            None,
        ),
        LoadingState::Loading => (
            vec![ListItem::new("Loading…").style(Style::default().fg(theme.dimmed))],
            None,
        ),
        LoadingState::Error(e) => (
            vec![ListItem::new(format!("Error: {e}")).style(Style::default().fg(accent))],
            None,
        ),
        LoadingState::Loaded(songs) => {
            if songs.is_empty() {
                (
                    vec![ListItem::new("No tracks").style(Style::default().fg(theme.dimmed))],
                    None,
                )
            } else {
                let items = songs
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let artist = s.artist.as_deref().unwrap_or("");
                        let dur = s.duration.map(fmt_duration_ms).unwrap_or_default();
                        let label = format!("{}. {}  {}  {}", i + 1, s.title, artist, dur);
                        ListItem::new(label).style(Style::default().fg(theme.foreground))
                    })
                    .collect();
                let sel = Some(overlay.selected_track_index.min(songs.len() - 1));
                (items, sel)
            }
        }
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(accent)
                .fg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ")
        .style(style_with_bg(theme.background));

    let mut state = ListState::default();
    state.select(sel);
    frame.render_stateful_widget(list, area, &mut state);
}

// ── Confirm dialog ────────────────────────────────────────────────────────────

fn render_confirm_dialog(
    frame: &mut Frame,
    parent: Rect,
    message: &str,
    accent: Color,
    theme: &Theme,
) {
    let w = (message.len() as u16 + 2).min(parent.width);
    let h = 3u16;
    let x = parent.x + (parent.width.saturating_sub(w)) / 2;
    let y = parent.y + (parent.height.saturating_sub(h)) / 2;
    let dialog_area = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    frame.render_widget(Clear, dialog_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(accent))
        .style(style_with_bg(theme.background));
    let para = Paragraph::new(Line::from(Span::styled(
        message,
        Style::default().fg(theme.foreground),
    )))
    .block(block);
    frame.render_widget(para, dialog_area);
}

// ── Playlist picker popup ─────────────────────────────────────────────────────

/// Render the floating playlist-picker popup (opened via `BrowserAddToPlaylist`).
pub fn render_playlist_picker(
    frame: &mut Frame,
    area: Rect,
    picker: &PlaylistPicker,
    accent: Color,
    theme: &Theme,
) {
    let w = (area.width / 2).max(30).min(area.width);
    let h = (area.height * 6 / 10).max(5).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let picker_area = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    frame.render_widget(Clear, picker_area);

    let (items, sel): (Vec<ListItem>, Option<usize>) = if picker.loading {
        (
            vec![ListItem::new("Loading…").style(Style::default().fg(theme.dimmed))],
            None,
        )
    } else if picker.playlists.is_empty() {
        (
            vec![ListItem::new("No playlists").style(Style::default().fg(theme.dimmed))],
            None,
        )
    } else {
        let items = picker
            .playlists
            .iter()
            .map(|p| ListItem::new(p.name.clone()).style(Style::default().fg(theme.foreground)))
            .collect();
        let sel = Some(picker.selected_index.min(picker.playlists.len() - 1));
        (items, sel)
    };

    let block = Block::default()
        .title(" Add to playlist  Enter: add  Esc: cancel ")
        .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(accent))
        .style(style_with_bg(theme.background));

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(accent)
                .fg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ")
        .style(style_with_bg(theme.background));

    let mut state = ListState::default();
    state.select(sel);
    frame.render_stateful_widget(list, picker_area, &mut state);
}
