//! Internet radio station picker (Shift+R) — play, browse, and manage stations.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::state::{LoadingState, RadioField, RadioInputMode};
use crate::theme::style_with_bg;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    if !app.radio.input_mode.is_normal() {
        render_form(app, frame, area);
        return;
    }

    let w = (area.width * 4 / 5).clamp(40, area.width);
    let h = (area.height * 7 / 10).clamp(8, area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, popup);

    let t = &app.theme;
    let accent = app.accent();
    let block = Block::default()
        .title(" Internet Radio ")
        .title_style(
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .style(style_with_bg(t.surface));

    match &app.radio.stations {
        LoadingState::NotLoaded | LoadingState::Loading => {
            let list = List::new(vec![ListItem::new("Loading stations…").style(
                Style::default().fg(t.dimmed),
            )])
            .block(block);
            frame.render_widget(list, popup);
        }
        LoadingState::Error(e) => {
            let list = List::new(vec![
                ListItem::new(format!("Error: {e}")).style(Style::default().fg(accent)),
                ListItem::new("Press r to retry · c to add a station").style(
                    Style::default().fg(t.dimmed),
                ),
            ])
            .block(block);
            frame.render_widget(list, popup);
        }
        LoadingState::Loaded(stations) => {
            if stations.is_empty() {
                let list = List::new(vec![
                    ListItem::new("No stations yet").style(Style::default().fg(t.dimmed)),
                    ListItem::new("Press c to add a station").style(Style::default().fg(t.dimmed)),
                ])
                .block(block);
                frame.render_widget(list, popup);
                return;
            }

            let playing_id = app
                .playback
                .current_song
                .as_ref()
                .filter(|s| App::is_radio_song(s))
                .and_then(|s| s.id.strip_prefix("radio:"));

            let visible_rows = popup.height.saturating_sub(2) as usize;
            let items: Vec<ListItem> = stations
                .iter()
                .enumerate()
                .skip(app.radio.scroll)
                .take(visible_rows.max(1))
                .map(|(i, station)| {
                    let global_idx = app.radio.scroll + i;
                    let buffering =
                        playing_id == Some(station.id.as_str()) && app.radio_buffering();
                    let playing = playing_id == Some(station.id.as_str())
                        && !app.playback.paused
                        && app.playback.player_loaded;
                    let marker = if playing {
                        "▶ "
                    } else if buffering {
                        "◌ "
                    } else {
                        "  "
                    };
                    let label = format!("{marker}{}", station.name);
                    let style = if global_idx == app.radio.selected {
                        Style::default()
                            .fg(accent)
                            .add_modifier(Modifier::BOLD)
                    } else if playing {
                        Style::default().fg(t.foreground)
                    } else {
                        Style::default().fg(t.dimmed)
                    };
                    ListItem::new(label).style(style)
                })
                .collect();

            let list = List::new(items).block(block);
            let mut state = ListState::default();
            if app.radio.selected >= app.radio.scroll {
                let local = app.radio.selected - app.radio.scroll;
                if local < visible_rows.max(1) {
                    state.select(Some(local));
                }
            }
            frame.render_stateful_widget(list, popup, &mut state);

            let hint = Paragraph::new(Line::from(vec![Span::styled(
                "Enter play · c add · e edit · X delete · r refresh · Esc close",
                Style::default().fg(t.dimmed),
            )]));
            let hint_area = Rect {
                x: popup.x + 1,
                y: popup.y + popup.height.saturating_sub(1),
                width: popup.width.saturating_sub(2),
                height: 1,
            };
            if hint_area.height > 0 && popup.height > 4 {
                frame.render_widget(hint, hint_area);
            }
        }
    }
}

fn render_form(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;
    let accent = app.accent();

    match &app.radio.input_mode {
        RadioInputMode::ConfirmingDelete { name, .. } => {
            let popup_w = (area.width * 3 / 4).clamp(32, area.width);
            let popup_h = 5u16.min(area.height);
            let popup = Rect {
                x: area.x + (area.width.saturating_sub(popup_w)) / 2,
                y: area.y + (area.height.saturating_sub(popup_h)) / 2,
                width: popup_w,
                height: popup_h,
            };
            frame.render_widget(Clear, popup);
            let block = Block::default()
                .title(" Delete station? ")
                .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(style_with_bg(t.surface));
            let text = format!("Delete \"{name}\"?\n\ny confirm · n cancel");
            let para = Paragraph::new(text).block(block);
            frame.render_widget(para, popup);
            return;
        }
        RadioInputMode::Creating { .. } | RadioInputMode::Editing { .. } => {
            let title = match &app.radio.input_mode {
                RadioInputMode::Creating { .. } => " New radio station ",
                RadioInputMode::Editing { .. } => " Edit radio station ",
                _ => unreachable!(),
            };
            let popup_w = (area.width * 4 / 5).clamp(40, area.width);
            let popup_h = 11u16.min(area.height);
            let popup = Rect {
                x: area.x + (area.width.saturating_sub(popup_w)) / 2,
                y: area.y + (area.height.saturating_sub(popup_h)) / 2,
                width: popup_w,
                height: popup_h,
            };
            frame.render_widget(Clear, popup);
            let block = Block::default()
                .title(title)
                .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(style_with_bg(t.surface));

            let focused = match &app.radio.input_mode {
                RadioInputMode::Creating { focused, .. }
                | RadioInputMode::Editing { focused, .. } => *focused,
                _ => RadioField::Name,
            };

            let (name, stream_url, home_page_url) = match &app.radio.input_mode {
                RadioInputMode::Creating {
                    name,
                    stream_url,
                    home_page_url,
                    ..
                }
                | RadioInputMode::Editing {
                    name,
                    stream_url,
                    home_page_url,
                    ..
                } => (name.as_str(), stream_url.as_str(), home_page_url.as_str()),
                _ => ("", "", ""),
            };

            let fields = [
                (RadioField::Name, name),
                (RadioField::StreamUrl, stream_url),
                (RadioField::HomePageUrl, home_page_url),
            ];

            let inner = block.inner(popup);
            frame.render_widget(block, popup);

            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(inner);

            for (i, (field, value)) in fields.iter().enumerate() {
                let is_focus = *field == focused;
                let label = format!("{}: ", field.label());
                let display = if value.is_empty() && is_focus {
                    format!("{label}_")
                } else if is_focus {
                    format!("{label}{value}_")
                } else {
                    format!("{label}{value}")
                };
                let style = if is_focus {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.dimmed)
                };
                frame.render_widget(Paragraph::new(display).style(style), rows[i]);
            }

            frame.render_widget(
                Paragraph::new("Tab next field · Enter save · Esc cancel").style(
                    Style::default().fg(t.dimmed),
                ),
                rows[3],
            );
        }
        RadioInputMode::Normal => {}
    }
}
