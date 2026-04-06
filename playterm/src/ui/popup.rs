//! Floating keybind reference popup.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::app::App;

/// Width reserved for the key column (padded with spaces to align descriptions).
const KEY_COL_W: usize = 12;

fn sections() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("Navigation", vec![
            ("j / k",       "Scroll up / down"),
            ("h / l",       "Previous / next column (Browser)"),
            ("1 / 2 / 3",   "Go to Home / Browse / Now Playing"),
            ("Tab",         "Next tab"),
            ("Shift-Tab",   "Previous tab"),
            ("/",           "Search current column"),
            ("Enter",       "Select / expand"),
        ]),
        ("Home Tab", vec![
            ("h / l",   "Select album"),
            ("j / k",   "Navigate list"),
            ("J / K",   "Switch section"),
            ("r",       "Re-roll rediscover"),
            ("Enter",   "Go to artist in Browse"),
        ]),
        ("Playback", vec![
            ("p / Space", "Play / pause"),
            ("n / N",     "Next / previous track"),
            ("x / Z",     "Shuffle / unshuffle"),
            ("\u{2190} / \u{2192}", "Seek \u{b1}10s"),
        ]),
        ("Queue", vec![
            ("a",       "Add track to queue"),
            ("A",       "On artist / album add to queue"),
            ("D",       "Clear queue"),
        ]),
        ("Library (fzf)", vec![
            ("Ctrl+f", "Open picker (Tab multi-select)"),
            ("Enter", "Append picks to queue"),
            ("Ctrl+r", "In picker: replace queue · else: refresh index"),
        ]),
        ("Volume & Display", vec![
            ("+ / -", "Volume up / down"),
            ("t",     "Toggle dynamic theme"),
            ("L",     "Toggle lyrics"),
            ("V",     "Toggle visualizer"),
        ]),
        ("App", vec![
            ("i", "Toggle this help"),
            ("q", "Quit (or close help)"),
        ]),
    ]
}

fn playlist_sections() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("Playlists", vec![
            ("Shift+P",    "Open / close playlist panel"),
            ("j / k",      "Scroll playlist / track list"),
            ("h / l",      "Switch between lists"),
            ("Enter",      "Play playlist / track"),
            ("Shift+A",    "Append playlist to queue"),
            (">",          "Add track to playlist (Browser)"),
            ("c",          "Create playlist"),
            ("r",          "Rename playlist"),
            ("X",          "Delete playlist (with confirm)"),
            ("<",          "Remove track from playlist"),
            ("Escape / q", "Close panel"),
        ]),
    ]
}

fn build_lines(
    sections: Vec<(&'static str, Vec<(&'static str, &'static str)>)>,
    accent: ratatui::style::Color,
    fg: ratatui::style::Color,
    dim: ratatui::style::Color,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (si, (header, entries)) in sections.into_iter().enumerate() {
        if si > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            header,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )));
        for (key, desc) in entries {
            let key_padded = format!("{:<width$}", key, width = KEY_COL_W);
            lines.push(Line::from(vec![
                Span::styled(key_padded, Style::default().fg(fg)),
                Span::styled(desc, Style::default().fg(dim)),
            ]));
        }
    }
    lines
}

/// Render the keybind help popup centered over the current frame.
///
/// Call this last in the render pass so it layers on top of all other widgets.
pub fn render_help(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let t = &app.theme;

    let accent = app.accent();
    let fg     = t.foreground;
    let dim    = t.dimmed;
    let bg     = t.background;

    // ── Build content for each column ─────────────────────────────────────────

    let left_lines  = build_lines(sections(),          accent, fg, dim);
    let right_lines = build_lines(playlist_sections(), accent, fg, dim);

    // ── Sizing & positioning ──────────────────────────────────────────────────

    let content_h = left_lines.len().max(right_lines.len()) as u16 + 2; // +2 for border
    let max_h     = (area.height * 80 / 100).max(10);
    let popup_h   = content_h.min(max_h);

    // Wide enough to comfortably hold two KEY_COL_W + desc columns side by side.
    let popup_w = (area.width * 70 / 100).max(80).min(area.width);

    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    // ── Render ────────────────────────────────────────────────────────────────

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(Span::styled(" Keybinds ", Style::default().fg(accent)))
        .padding(Padding::horizontal(4))
        .style(Style::default().bg(bg));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Split inner area into two equal columns.
    let cols = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(inner);

    let left_para = Paragraph::new(Text::from(left_lines))
        .style(Style::default().bg(bg));
    frame.render_widget(left_para, cols[0]);

    let right_para = Paragraph::new(Text::from(right_lines))
        .style(Style::default().bg(bg));
    frame.render_widget(right_para, cols[1]);
}
