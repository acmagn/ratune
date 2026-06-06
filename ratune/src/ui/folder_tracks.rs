use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, BrowserColumn};
use crate::config::BrowseMode;
use crate::state::{folder_preview_rows, FolderBrowseState, FolderPreviewRow, LoadingState};
use crate::theme::style_with_bg;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect, is_active: bool) {
    let t = &app.theme;
    let border_color = if is_active { app.accent() } else { t.border };
    let title_color = if is_active { app.accent() } else { t.dimmed };

    let border_style = if is_active {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };

    let preview_title: String = match &app.folders.preview_dir_id {
        Some(id) => match app.folders.listings.get(id) {
            Some(LoadingState::Loaded(li)) => {
                let n = li.name.trim();
                if n.is_empty() {
                    " Preview ".to_string()
                } else {
                    format!(" {n} ")
                }
            }
            _ => " Preview ".to_string(),
        },
        None => " Preview ".to_string(),
    };

    let block = Block::default()
        .title(preview_title.as_str())
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style)
        .style(style_with_bg(t.surface));

    if app.browser_browse_mode != BrowseMode::Files {
        let list =
            List::new(vec![ListItem::new("—").style(Style::default().fg(t.dimmed))]).block(block);
        frame.render_widget(list, area);
        return;
    }

    let Some(ref pid) = app.folders.preview_dir_id.clone() else {
        let msg = if app.folders.path.is_empty() {
            "← Select a library folder"
        } else {
            "No preview"
        };
        let list =
            List::new(vec![ListItem::new(msg).style(Style::default().fg(t.dimmed))]).block(block);
        frame.render_widget(list, area);
        return;
    };

    match app.folders.listings.get(pid) {
        None | Some(LoadingState::NotLoaded) | Some(LoadingState::Loading) => {
            let item = ListItem::new("Loading…").style(Style::default().fg(t.dimmed));
            let list = List::new(vec![item]).block(block);
            frame.render_widget(list, area);
        }
        Some(LoadingState::Error(e)) => {
            let item =
                ListItem::new(format!("Error: {e}")).style(Style::default().fg(app.accent()));
            let list = List::new(vec![item]).block(block);
            frame.render_widget(list, area);
        }
        Some(LoadingState::Loaded(listing)) => {
            let make_track_label = |s: &ratune_subsonic::Song| {
                let dur = s
                    .duration
                    .map(|d| {
                        let m = d / 60;
                        let sec = d % 60;
                        format!("  {m}:{sec:02}")
                    })
                    .unwrap_or_default();
                format!("{}{}", s.title, dur)
            };

            let rows =
                folder_preview_rows(listing, app.browser_column_filter(BrowserColumn::Tracks));

            let visible: Vec<(FolderPreviewRow, String)> = rows
                .into_iter()
                .map(|row| {
                    let label = match row {
                        FolderPreviewRow::Dir(i) => {
                            format!("📁 {}", listing.directories[i].1)
                        }
                        FolderPreviewRow::Track(i) => make_track_label(&listing.tracks[i]),
                    };
                    (row, label)
                })
                .collect();

            let items: Vec<ListItem> = if visible.is_empty() {
                let msg = if app.browser_column_filter(BrowserColumn::Tracks).is_some() {
                    "No matches"
                } else {
                    "Empty folder"
                };
                vec![ListItem::new(msg).style(Style::default().fg(t.dimmed))]
            } else {
                visible
                    .iter()
                    .map(|(_, label)| {
                        ListItem::new(label.as_str()).style(Style::default().fg(t.foreground))
                    })
                    .collect()
            };

            let sel_ix = if visible.is_empty() {
                None
            } else {
                Some(
                    app.folders
                        .preview_selected_row
                        .min(visible.len().saturating_sub(1)),
                )
            };

            let list = List::new(items)
                .block(block)
                .highlight_style(
                    Style::default()
                        .bg(app.accent())
                        .fg(t.background)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▶ ")
                .style(style_with_bg(t.surface));

            let vh = area.height.saturating_sub(2) as usize;
            let vh = vh.max(1);
            app.browser_list_viewport_rows = vh;

            let mut state = ListState::default();
            if !visible.is_empty() {
                if let Some(sel_ix) = sel_ix {
                    FolderBrowseState::clamp_scroll(
                        &mut app.folders.tracks_scroll,
                        sel_ix,
                        visible.len(),
                        vh,
                    );
                    state = ListState::default().with_offset(app.folders.tracks_scroll);
                    state.select(Some(sel_ix));
                } else {
                    let max_first = visible.len().saturating_sub(vh);
                    app.folders.tracks_scroll = app.folders.tracks_scroll.min(max_first);
                    state = ListState::default().with_offset(app.folders.tracks_scroll);
                }
            }
            frame.render_stateful_widget(list, area, &mut state);
        }
    }
}
