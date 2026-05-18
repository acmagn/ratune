//! Floating keybind reference popup.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::app::App;

/// Width reserved for the key column (padded with spaces to align descriptions).
const KEY_COL_W: usize = 12;

fn sections() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        (
            "Navigation",
            vec![
                ("j / k", "Scroll up / down"),
                ("h / l", "Previous / next column (Browser)"),
                ("1 / 2 / 3", "Go to Home / Browse / Now Playing"),
                ("Tab", "Next tab"),
                ("Shift-Tab", "Previous tab"),
                ("/", "Search column (Enter apply · Esc/Ctrl+C clear filter)"),
                ("Enter", "Select / expand"),
                (
                    "Ctrl+b",
                    "Browse: toggle folder view (if enabled in config)",
                ),
            ],
        ),
        (
            "Home Tab",
            vec![
                ("h / l", "Select album"),
                ("j / k", "Navigate list"),
                ("J / K", "Switch section"),
                ("r", "Re-roll rediscover"),
                ("Enter", "Go to artist in Browse"),
            ],
        ),
        (
            "Playback",
            vec![
                ("p / Space", "Play / pause"),
                ("n / N", "Next / previous track"),
                ("x / Z", "Shuffle / unshuffle"),
                ("\u{2190} / \u{2192}", "Seek \u{b1}10s"),
            ],
        ),
        (
            "Queue",
            vec![
                ("a", "Add track to queue"),
                ("A", "Add all (artist/album or folder preview)"),
                ("Ctrl+r", "Replace queue with album or folder preview"),
                ("Ctrl+a", "Append full index to queue (y/n)"),
                ("D", "Clear queue"),
            ],
        ),
        (
            "Library (fzf)",
            vec![
                ("Ctrl+f", "Open picker (Tab multi-select)"),
                ("Enter", "Append picks to queue"),
                ("Ctrl+r", "In picker: replace queue · else: refresh index"),
            ],
        ),
        (
            "Volume & Display",
            vec![
                (
                    "+ / -",
                    "In-app level (duck music under games); saved on quit",
                ),
                ("t", "Toggle dynamic theme"),
                ("L", "Toggle lyrics"),
                ("V", "Toggle visualizer"),
            ],
        ),
        (
            "App",
            vec![("i", "Toggle this help"), ("q", "Quit (or close help)")],
        ),
    ]
}

fn playlist_sections() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![(
        "Playlists",
        vec![
            ("Shift+P", "Open / close playlist panel"),
            ("j / k", "Scroll playlist / track list"),
            ("h / l", "Switch between lists"),
            ("Enter", "Play playlist / track"),
            ("Shift+A", "Append playlist to queue"),
            (">", "Add track to playlist (Browser)"),
            ("c", "Create playlist"),
            ("r", "Rename playlist"),
            ("X", "Delete playlist (with confirm)"),
            ("<", "Remove track from playlist"),
            ("Escape / q", "Close panel"),
        ],
    )]
}

fn build_blocks(
    sections: Vec<(&'static str, Vec<(&'static str, &'static str)>)>,
    accent: ratatui::style::Color,
    fg: ratatui::style::Color,
    dim: ratatui::style::Color,
) -> Vec<Vec<Line<'static>>> {
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
    for (si, (header, entries)) in sections.into_iter().enumerate() {
        let mut b: Vec<Line<'static>> = Vec::new();
        if si > 0 {
            b.push(Line::from(""));
        }
        b.push(Line::from(Span::styled(
            header,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )));
        for (key, desc) in entries {
            let key_padded = format!("{:<width$}", key, width = KEY_COL_W);
            b.push(Line::from(vec![
                Span::styled(key_padded, Style::default().fg(fg)),
                Span::styled(desc, Style::default().fg(dim)),
            ]));
        }
        blocks.push(b);
    }
    blocks
}

fn pack_blocks_into_two_columns(
    blocks: Vec<Vec<Line<'static>>>,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let total: usize = blocks.iter().map(|b| b.len()).sum();
    let target_left = total.div_ceil(2);

    let mut left: Vec<Line<'static>> = Vec::new();
    let mut right: Vec<Line<'static>> = Vec::new();
    let mut left_h = 0usize;
    let mut on_right = false;

    for b in blocks {
        let bh = b.len();
        if !on_right && left_h > 0 && left_h + bh > target_left {
            on_right = true;
        }
        if on_right {
            right.extend(b);
        } else {
            left.extend(b);
            left_h += bh;
        }
    }
    (left, right)
}

/// Render the keybind help popup centered over the current frame.
///
/// Call this last in the render pass so it layers on top of all other widgets.
pub fn render_help(app: &mut App, frame: &mut Frame) {
    let area = frame.area();
    let t = &app.theme;

    let accent = app.accent();
    let fg = t.foreground;
    let dim = t.dimmed;
    let bg = t.background;

    // ── Build content as section blocks, pack into two columns ────────────────
    // Keep each category within one column where possible, while aiming for
    // roughly equal column heights.

    let mut blocks = build_blocks(sections(), accent, fg, dim);
    blocks.push(vec![Line::from("")]);
    blocks.extend(build_blocks(playlist_sections(), accent, fg, dim));
    let (left_all, right_all) = pack_blocks_into_two_columns(blocks);

    // ── Sizing & positioning ──────────────────────────────────────────────────

    // Size to fit the taller column, but clamp to a percentage of the terminal.
    let required_inner_h = left_all.len().max(right_all.len()).max(1) as u16;
    let content_h = required_inner_h + 2; // +2 for border
    let max_h = (area.height * 80 / 100).max(10);
    let popup_h = content_h.min(max_h);

    // Wide enough to comfortably hold two KEY_COL_W + desc columns side by side.
    let popup_w = (area.width * 70 / 100).max(80).min(area.width);

    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    // ── Render ────────────────────────────────────────────────────────────────

    frame.render_widget(Clear, popup_area);

    // Split inner area into two equal columns.
    let inner = Rect {
        x: popup_area.x + 1,
        y: popup_area.y + 1,
        width: popup_area.width.saturating_sub(2),
        height: popup_area.height.saturating_sub(2),
    };

    // Shared scroll offset: both columns move together.
    let col_h = inner.height.max(1) as usize;
    let max_col_len = left_all.len().max(right_all.len());
    let max_scroll = max_col_len.saturating_sub(col_h);
    // Clamp and write back so we never accumulate invisible overscroll.
    app.help_scroll = app.help_scroll.min(max_scroll);
    let scroll = app.help_scroll;

    let start_line_1 = if max_col_len == 0 {
        0usize
    } else {
        scroll.saturating_add(1)
    };
    let end_line_1 = (scroll + col_h).min(max_col_len);
    let right_title = format!(
        " {}–{}/{}  ·  j/k or ↑/↓ scroll  ·  i/q/esc close ",
        start_line_1, end_line_1, max_col_len
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title_top(
            Line::from(Span::styled(" Keybinds ", Style::default().fg(accent))).left_aligned(),
        )
        .title_top(
            Line::from(Span::styled(right_title, Style::default().fg(accent))).right_aligned(),
        )
        .padding(Padding::horizontal(4))
        .style(Style::default().bg(bg));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(inner);

    let left_lines: Vec<Line<'static>> =
        left_all.iter().skip(scroll).take(col_h).cloned().collect();
    let right_lines: Vec<Line<'static>> =
        right_all.iter().skip(scroll).take(col_h).cloned().collect();

    let left_para = Paragraph::new(Text::from(left_lines)).style(Style::default().bg(bg));
    frame.render_widget(left_para, cols[0]);

    let right_para = Paragraph::new(Text::from(right_lines)).style(Style::default().bg(bg));
    frame.render_widget(right_para, cols[1]);
}
