use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use super::now_playing_format::{format_now_playing_line, NowPlayingContext};

use crate::action::Action;
use crate::app::{App, Tab};
use crate::theme::style_with_bg;

/// Spacing between transport glyphs in the controls row (must match `render_controls_widget`).
const CONTROLS_GAP: &str = "      ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlSlot {
    Shuffle,
    Prev,
    PlayPause,
    Next,
    Loop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ControlSegment {
    slot: ControlSlot,
    /// Display-column offset from the start of the centered controls line.
    start: u16,
    width: u16,
}

/// Ratatui `Paragraph` center alignment (`widgets/paragraph.rs::get_line_offset`).
fn centered_line_x(area_x: u16, area_width: u16, line_width: u16) -> u16 {
    area_x + (area_width / 2).saturating_sub(line_width / 2)
}

fn slot_for_span_index(i: usize) -> Option<ControlSlot> {
    match i {
        0 => Some(ControlSlot::Shuffle),
        2 => Some(ControlSlot::Prev),
        4 => Some(ControlSlot::PlayPause),
        6 => Some(ControlSlot::Next),
        8 => Some(ControlSlot::Loop),
        _ => None,
    }
}

/// Layout derived from the same `Line` ratatui renders (width + per-span columns).
fn control_segments(ctx: ControlsClickCtx) -> (u16, Vec<ControlSegment>) {
    let line = controls_line_plain(ctx);
    let total = line.width() as u16;
    let mut pos = 0u16;
    let mut segments = Vec::with_capacity(5);
    for (i, span) in line.spans.iter().enumerate() {
        let width = span.width() as u16;
        if let Some(slot) = slot_for_span_index(i) {
            segments.push(ControlSegment {
                slot,
                start: pos,
                width,
            });
        }
        pos += width;
    }
    (total, segments)
}

fn controls_line_plain(ctx: ControlsClickCtx) -> Line<'static> {
    let play_label = play_label_for(ctx);
    Line::from(vec![
        Span::raw("⇄"),
        Span::raw(CONTROLS_GAP),
        Span::raw("⏮"),
        Span::raw(CONTROLS_GAP),
        Span::raw(play_label),
        Span::raw(CONTROLS_GAP),
        Span::raw("⏭"),
        Span::raw(CONTROLS_GAP),
        Span::raw("↻"),
    ])
}

fn action_for_slot(slot: ControlSlot, ctx: ControlsClickCtx) -> Action {
    match slot {
        ControlSlot::Shuffle => {
            if ctx.shuffled {
                Action::Unshuffle
            } else {
                Action::Shuffle
            }
        }
        ControlSlot::Prev => Action::PrevTrack,
        ControlSlot::PlayPause => Action::PlayPause,
        ControlSlot::Next => Action::NextTrack,
        ControlSlot::Loop => Action::ToggleQueueLoop,
    }
}

/// Context for control-line layout / hit-testing without a full `App`.
#[derive(Debug, Clone, Copy)]
pub struct ControlsClickCtx {
    pub has_current_song: bool,
    pub paused: bool,
    pub shuffled: bool,
}

impl ControlsClickCtx {
    pub fn from_app(app: &App) -> Self {
        Self {
            has_current_song: app.playback.current_song.is_some(),
            paused: app.playback.paused,
            shuffled: app.queue.is_shuffled(),
        }
    }
}

fn play_label_for(ctx: ControlsClickCtx) -> &'static str {
    if !ctx.has_current_song {
        "▶"
    } else if ctx.paused {
        "( ▶ )"
    } else {
        "( ⏸ )"
    }
}

/// Total display width of the centered controls line (used by click tests).
#[cfg(test)]
fn controls_line_width(ctx: ControlsClickCtx) -> u16 {
    controls_line_plain(ctx).width() as u16
}

/// The single terminal row that contains the interactive transport glyphs.
pub fn controls_row_rect(area: Rect) -> Rect {
    let y = if area.height <= 1 { area.y } else { area.y + 1 };
    Rect::new(area.x, y, area.width, 1)
}

/// Map a terminal column inside `controls_area` to a transport action, if any.
pub fn controls_click_action(app: &App, controls_area: Rect, x: u16) -> Option<Action> {
    controls_click_action_for(ControlsClickCtx::from_app(app), controls_area, x)
}

pub fn controls_click_action_for(
    ctx: ControlsClickCtx,
    controls_area: Rect,
    x: u16,
) -> Option<Action> {
    let row = controls_row_rect(controls_area);
    if x < row.x || x >= row.x + row.width {
        return None;
    }

    let (total, segments) = control_segments(ctx);
    if total == 0 || row.width == 0 {
        return None;
    }
    let start = centered_line_x(row.x, row.width, total);
    if x < start || x >= start + total {
        return None;
    }
    let cx = x - start;

    for seg in segments {
        if cx >= seg.start && cx < seg.start + seg.width {
            return Some(action_for_slot(seg.slot, ctx));
        }
    }
    None
}

#[cfg(test)]
fn segment_center(segments: &[ControlSegment], slot: ControlSlot) -> u16 {
    let seg = segments.iter().find(|s| s.slot == slot).expect("segment");
    seg.start + seg.width / 2
}

/// Blank rows between the block title bar and the first metadata line (boxed pane only).
const BOXED_TITLE_GAP_ROWS: u16 = 1;

/// Line count for boxed metadata (gap + template lines or “Not playing”), for layout + hit-testing.
fn boxed_meta_line_count(app: &App) -> u16 {
    let templates = template_lines_boxed(app);
    if app.playback.current_song.is_none() {
        return BOXED_TITLE_GAP_ROWS + 1;
    }
    (templates.len().max(1) as u16).saturating_add(BOXED_TITLE_GAP_ROWS)
}

// ── Public: hit testing (keep in sync with `render`) ─────────────────────────

/// Regions for transport / seek clicks. `None` when that chrome is hidden.
pub struct NowPlayingChromeRects {
    pub controls: Option<Rect>,
    pub progress: Option<Rect>,
}

pub fn interaction_rects(app: &App, np: Rect) -> NowPlayingChromeRects {
    if np.width == 0 || np.height == 0 {
        return NowPlayingChromeRects {
            controls: None,
            progress: None,
        };
    }

    let boxed = app
        .config
        .now_playing_layout
        .trim()
        .eq_ignore_ascii_case("boxed");

    // On Now Playing tab the metadata lives in the center dock; the strip is footer-only.
    // Lyrics use that dock, so we keep row-style chrome in the bottom strip for hit-testing.
    if boxed && app.active_tab == Tab::NowPlaying && !app.lyrics_visible {
        chrome_rects_boxed_bottom_bar(app, np)
    } else {
        chrome_rects_row(app, np)
    }
}

/// Hit-testing for controls/progress drawn inside the boxed pane (Now Playing tab center dock).
pub fn interaction_rects_pane(app: &App, pane: Rect) -> NowPlayingChromeRects {
    if pane.width == 0 || pane.height == 0 {
        return NowPlayingChromeRects {
            controls: None,
            progress: None,
        };
    }

    let t = &app.theme;
    let accent = app.accent();
    let show_c = app.config.now_playing_show_controls;
    let show_p = app.config.now_playing_show_progress;
    let c_in = app.config.now_playing_box_include_controls;
    let p_in = app.config.now_playing_box_include_progress;

    let block = Block::default()
        .title(" Now Playing ")
        .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(accent))
        .style(style_with_bg(t.surface));

    let inner = block.inner(pane);
    chrome_rects_boxed_inner(inner, app, show_c, show_p, c_in, p_in)
}

fn chrome_rects_row(app: &App, area: Rect) -> NowPlayingChromeRects {
    let show_c = app.config.now_playing_show_controls;
    let show_p = app.config.now_playing_show_progress;

    let cols = split_row_columns(area, show_c, show_p);

    let controls = if show_c {
        cols.controls.map(controls_row_rect)
    } else {
        None
    };

    let progress = if show_p {
        cols.progress.map(progress_row_rect)
    } else {
        None
    };

    NowPlayingChromeRects { controls, progress }
}

/// Footer-only controls/progress in the global bottom `now_playing` strip (boxed layout).
/// Content is bottom-aligned in `area` (flush above the tab bar), with a spacer row between
/// controls and progress when both are shown.
fn chrome_rects_boxed_bottom_bar(app: &App, area: Rect) -> NowPlayingChromeRects {
    let show_c = app.config.now_playing_show_controls;
    let show_p = app.config.now_playing_show_progress;
    let c_in = app.config.now_playing_box_include_controls;
    let p_in = app.config.now_playing_box_include_progress;

    let show_c_out = show_c && !c_in;
    let show_p_out = show_p && !p_in;

    let mut fh = u16::from(show_c_out) + u16::from(show_p_out);
    if show_c_out && show_p_out {
        fh += 1;
    }
    if fh == 0 {
        return NowPlayingChromeRects {
            controls: None,
            progress: None,
        };
    }

    let y0 = area.y + area.height.saturating_sub(fh);
    let mut y = y0;

    let controls = if show_c_out {
        let r = Rect::new(area.x, y, area.width, 1);
        y += 1;
        if show_c_out && show_p_out {
            y += 1;
        }
        Some(r)
    } else {
        None
    };

    let progress = if show_p_out {
        Some(Rect::new(area.x, y, area.width, 1))
    } else {
        None
    };

    NowPlayingChromeRects { controls, progress }
}

fn chrome_rects_boxed_inner(
    inner: Rect,
    app: &App,
    show_c: bool,
    show_p: bool,
    c_in: bool,
    p_in: bool,
) -> NowPlayingChromeRects {
    if inner.width == 0 || inner.height == 0 {
        return NowPlayingChromeRects {
            controls: None,
            progress: None,
        };
    }

    let meta_lines_len = boxed_meta_line_count(app);

    let reserved = u16::from(show_c && c_in) + u16::from(show_p && p_in);
    let meta_h = {
        let need = meta_lines_len.saturating_add(reserved);
        if need > inner.height {
            inner.height.saturating_sub(reserved)
        } else {
            meta_lines_len.min(inner.height.saturating_sub(reserved))
        }
    };

    let mut controls = None;
    let mut progress = None;

    if show_c && c_in {
        let y = inner.y + meta_h;
        controls = Some(Rect::new(inner.x, y, inner.width, 1));
    }
    if show_p && p_in {
        let y = inner.y + meta_h + u16::from(show_c && c_in);
        progress = Some(Rect::new(inner.x, y, inner.width, 1));
    }

    NowPlayingChromeRects { controls, progress }
}

// ── Render ───────────────────────────────────────────────────────────────────

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let boxed = app
        .config
        .now_playing_layout
        .trim()
        .eq_ignore_ascii_case("boxed");

    if boxed && app.active_tab == Tab::NowPlaying && !app.lyrics_visible {
        render_boxed_bottom_bar_only(app, frame, area);
    } else {
        render_row(app, frame, area);
    }
}

/// Bordered metadata + optional inline controls/progress inside `pane` (Visualizer-style dock).
pub fn render_boxed_pane(app: &App, frame: &mut Frame, pane: Rect) {
    if pane.width == 0 || pane.height == 0 {
        return;
    }

    let t = &app.theme;
    let accent = app.accent();
    let show_c = app.config.now_playing_show_controls;
    let show_p = app.config.now_playing_show_progress;
    let c_in = app.config.now_playing_box_include_controls;
    let p_in = app.config.now_playing_box_include_progress;

    let block = Block::default()
        .title(" Now Playing ")
        .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(accent))
        .style(style_with_bg(t.surface));

    let inner = block.inner(pane);
    frame.render_widget(block, pane);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let meta_lines_len = boxed_meta_line_count(app);

    let ctx = np_context(app);
    let templates = template_lines_boxed(app);
    let mut meta_lines: Vec<Line> = if app.playback.current_song.is_none() {
        vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                "Not playing",
                Style::default().fg(t.dimmed),
            )]),
        ]
    } else {
        let mut lines: Vec<Line> = vec![Line::from("")];
        lines.extend(
            templates
                .iter()
                .map(|tpl| format_now_playing_line(tpl, &ctx, t, accent)),
        );
        lines
    };

    let reserved = u16::from(show_c && c_in) + u16::from(show_p && p_in);
    let meta_h = {
        let need = meta_lines_len.saturating_add(reserved);
        if need > inner.height {
            inner.height.saturating_sub(reserved)
        } else {
            meta_lines_len.min(inner.height.saturating_sub(reserved))
        }
    };

    meta_lines.truncate(meta_h as usize);

    let meta_rect = Rect::new(inner.x, inner.y, inner.width, meta_h);
    frame.render_widget(
        Paragraph::new(meta_lines).alignment(Alignment::Left),
        meta_rect,
    );

    let mut y = meta_rect.y + meta_h;

    if show_c && c_in {
        let r = Rect::new(inner.x, y, inner.width, 1);
        render_controls_widget(app, frame, r);
        y += 1;
    }
    if show_p && p_in {
        let r = Rect::new(inner.x, y, inner.width, 1);
        render_progress_widget(app, frame, r);
    }
}

/// Global bottom strip when `boxed`: optional footer controls/progress only (metadata lives in pane).
/// Footer rows are bottom-aligned in `area` with a spacer between controls and progress.
fn render_boxed_bottom_bar_only(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;
    let show_c = app.config.now_playing_show_controls;
    let show_p = app.config.now_playing_show_progress;
    let c_in = app.config.now_playing_box_include_controls;
    let p_in = app.config.now_playing_box_include_progress;

    let show_c_out = show_c && !c_in;
    let show_p_out = show_p && !p_in;

    let mut fh = u16::from(show_c_out) + u16::from(show_p_out);
    if show_c_out && show_p_out {
        fh += 1;
    }

    frame.render_widget(
        Paragraph::new(Line::from("")).style(style_with_bg(t.surface)),
        area,
    );

    if fh == 0 {
        return;
    }

    let y0 = area.y + area.height.saturating_sub(fh);
    let footer = Rect::new(area.x, y0, area.width, fh);

    if show_c_out && show_p_out {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(footer);
        render_controls_widget(app, frame, rows[0]);
        frame.render_widget(
            Paragraph::new(Line::from("")).style(style_with_bg(t.surface)),
            rows[1],
        );
        render_progress_widget(app, frame, rows[2]);
    } else if show_c_out {
        render_controls_widget(app, frame, footer);
    } else if show_p_out {
        render_progress_widget(app, frame, footer);
    }
}

struct RowColumns {
    info: Rect,
    controls: Option<Rect>,
    progress: Option<Rect>,
}

fn split_row_columns(area: Rect, show_c: bool, show_p: bool) -> RowColumns {
    match (show_c, show_p) {
        (true, true) => {
            let c = Layout::horizontal([
                Constraint::Percentage(30),
                Constraint::Percentage(40),
                Constraint::Percentage(30),
            ])
            .split(area);
            RowColumns {
                info: c[0],
                controls: Some(c[1]),
                progress: Some(c[2]),
            }
        }
        (true, false) => {
            let c = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            RowColumns {
                info: c[0],
                controls: Some(c[1]),
                progress: None,
            }
        }
        (false, true) => {
            let c = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            RowColumns {
                info: c[0],
                controls: None,
                progress: Some(c[1]),
            }
        }
        (false, false) => RowColumns {
            info: area,
            controls: None,
            progress: None,
        },
    }
}

fn builtin_lines(ratings_enabled: bool) -> Vec<String> {
    if ratings_enabled {
        vec!["$b%t$/b  %R".into(), "%a".into(), "%b".into()]
    } else {
        vec!["$b%t$/b".into(), "%a".into(), "%b".into()]
    }
}

fn template_lines_row(app: &App) -> Vec<String> {
    let v = &app.config.now_playing_lines_row;
    if v.is_empty() {
        builtin_lines(app.config.ratings_enabled)
    } else {
        v.clone()
    }
}

fn template_lines_boxed(app: &App) -> Vec<String> {
    let v = &app.config.now_playing_lines_boxed;
    if v.is_empty() {
        builtin_lines(app.config.ratings_enabled)
    } else {
        v.clone()
    }
}

fn queue_total_duration_secs(queue: &crate::state::QueueState) -> Option<u64> {
    if queue.songs.is_empty() {
        return None;
    }
    let mut sum = 0u64;
    let mut any = false;
    for s in &queue.songs {
        if let Some(d) = s.duration {
            sum += u64::from(d);
            any = true;
        }
    }
    any.then_some(sum)
}

/// 1-based index of the current track in the queue and total count (for `%i` / `%j`).
fn queue_position_now_playing(app: &App) -> Option<(usize, usize)> {
    let n = app.queue.songs.len();
    if n == 0 {
        return None;
    }
    let current = app.playback.current_song.as_ref()?;
    let idx = app
        .queue
        .songs
        .iter()
        .position(|s| s.id == current.id)
        .unwrap_or_else(|| app.queue.cursor.min(n.saturating_sub(1)));
    Some((idx + 1, n))
}

fn np_context(app: &App) -> NowPlayingContext<'_> {
    let is_radio = app
        .playback
        .current_song
        .as_ref()
        .is_some_and(App::is_radio_song);
    NowPlayingContext {
        song: app.playback.current_song.as_ref(),
        elapsed: app.playback.elapsed,
        total: app.playback.total,
        paused: app.playback.paused,
        volume_percent: app.config.default_volume,
        queue_total_duration_secs: if is_radio {
            None
        } else {
            queue_total_duration_secs(&app.queue)
        },
        queue_position: if is_radio {
            None
        } else {
            queue_position_now_playing(app)
        },
        ratings_enabled: app.config.ratings_enabled,
        rating_stars: &app.config.rating_stars,
    }
}

fn render_row(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;
    let accent = app.accent();
    let show_c = app.config.now_playing_show_controls;
    let show_p = app.config.now_playing_show_progress;

    let cols = split_row_columns(area, show_c, show_p);
    let ctx = np_context(app);

    let templates = template_lines_row(app);
    let lines: Vec<Line> = if app.playback.current_song.is_none() {
        vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("Not playing", Style::default().fg(t.dimmed)),
            ]),
        ]
    } else {
        templates
            .iter()
            .map(|tpl| format_now_playing_line(tpl, &ctx, t, accent))
            .collect()
    };

    let mut padded = vec![Line::from("")];
    padded.extend(lines);
    while padded.len() < area.height as usize {
        padded.push(Line::from(""));
    }

    frame.render_widget(
        Paragraph::new(padded).style(style_with_bg(t.surface)),
        cols.info,
    );

    if show_c {
        if let Some(ca) = cols.controls {
            render_controls_widget(app, frame, ca);
        }
    }
    if show_p {
        if let Some(pa) = cols.progress {
            render_progress_widget(app, frame, pa);
        }
    }
}

// ── Controls & progress (shared row + boxed) ─────────────────────────────────

fn progress_row_rect(area: Rect) -> Rect {
    let y = if area.height <= 1 { area.y } else { area.y + 2 };
    Rect::new(area.x, y, area.width, 1)
}

fn render_controls_widget(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;
    let ctx = ControlsClickCtx::from_app(app);
    let (play_label, play_style) = if !ctx.has_current_song {
        ("▶", Style::default().fg(t.dimmed))
    } else if ctx.paused {
        (
            "( ▶ )",
            Style::default()
                .fg(app.accent())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            "( ⏸ )",
            Style::default()
                .fg(app.accent())
                .add_modifier(Modifier::BOLD),
        )
    };

    let inactive = Style::default().fg(t.dimmed);
    let active = Style::default()
        .fg(app.accent())
        .add_modifier(Modifier::BOLD);
    let shuffle_style = if ctx.shuffled { active } else { inactive };
    let loop_style = if app.queue.loop_enabled {
        active
    } else {
        inactive
    };

    let controls = Line::from(vec![
        Span::styled("⇄", shuffle_style),
        Span::raw(CONTROLS_GAP),
        Span::styled("⏮", inactive),
        Span::raw(CONTROLS_GAP),
        Span::styled(play_label, play_style),
        Span::raw(CONTROLS_GAP),
        Span::styled("⏭", inactive),
        Span::raw(CONTROLS_GAP),
        Span::styled("↻", loop_style),
    ]);

    if area.height <= 1 {
        frame.render_widget(
            Paragraph::new(controls)
                .alignment(Alignment::Center)
                .style(style_with_bg(t.surface)),
            area,
        );
        return;
    }

    let lines = vec![Line::from(""), controls, Line::from(""), Line::from("")];

    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(style_with_bg(t.surface)),
        area,
    );
}

#[derive(Clone, Copy)]
enum ProgressStyleSpec {
    Hidden,
    FractionalBlocks { filled: char, empty: char },
    Ncmpcpp(char, char, char),
}

fn default_fractional_spec() -> ProgressStyleSpec {
    ProgressStyleSpec::FractionalBlocks {
        filled: '█',
        empty: '░',
    }
}

fn parse_progress_style(raw: &str) -> ProgressStyleSpec {
    let t = raw.trim();
    if t.is_empty() {
        return ProgressStyleSpec::Hidden;
    }
    let chs: Vec<char> = t.chars().collect();
    if chs.len() == 3 {
        if chs[0] == chs[1] {
            return ProgressStyleSpec::FractionalBlocks {
                filled: chs[0],
                empty: chs[2],
            };
        }
        return ProgressStyleSpec::Ncmpcpp(chs[0], chs[1], chs[2]);
    }
    default_fractional_spec()
}

fn build_progress_bar(
    spec: ProgressStyleSpec,
    ratio: f64,
    bar_w: usize,
) -> (String, String, String) {
    if bar_w == 0 {
        return (String::new(), String::new(), String::new());
    }
    let ratio = ratio.clamp(0.0, 1.0);
    match spec {
        ProgressStyleSpec::Hidden => (String::new(), String::new(), String::new()),
        ProgressStyleSpec::FractionalBlocks { filled, empty } => {
            const FRAC: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
            let units = ((ratio * bar_w as f64 * 8.0) as usize).min(bar_w * 8);
            let full = units / 8;
            let frac = units % 8;
            let has_partial = frac > 0 && full < bar_w;
            let empty_n = bar_w - full - usize::from(has_partial);
            let filled_str = std::iter::repeat_n(filled, full).collect();
            let partial_str = if has_partial {
                FRAC[frac - 1].to_string()
            } else {
                String::new()
            };
            let empty_str = std::iter::repeat_n(empty, empty_n).collect();
            (filled_str, partial_str, empty_str)
        }
        ProgressStyleSpec::Ncmpcpp(f, c, e) => {
            let filled_n = (ratio * bar_w as f64).floor() as usize;
            let filled_n = filled_n.min(bar_w);
            if filled_n >= bar_w {
                (
                    std::iter::repeat_n(f, bar_w).collect(),
                    String::new(),
                    String::new(),
                )
            } else {
                let rest = bar_w - filled_n - 1;
                (
                    std::iter::repeat_n(f, filled_n).collect(),
                    c.to_string(),
                    std::iter::repeat_n(e, rest).collect(),
                )
            }
        }
    }
}

fn render_live_progress_line(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    t: &crate::theme::Theme,
    accent_color: ratatui::style::Color,
    label: &str,
    right: String,
    animate: bool,
) {
    let col_w = area.width as usize;
    let bar_w = col_w.saturating_sub(label.len() + right.len() + 4);
    let ratio = if animate {
        0.35 + 0.1 * ((app.playback.elapsed.as_secs() % 4) as f64 / 4.0)
    } else {
        0.0
    };
    let spec = parse_progress_style(&app.config.progress_style);
    let (filled_str, partial_str, empty_str) = build_progress_bar(spec, ratio, bar_w);
    let progress = Line::from(vec![
        Span::styled(label, Style::default().fg(accent_color)),
        Span::raw("  "),
        Span::styled(filled_str, Style::default().fg(accent_color)),
        Span::styled(partial_str, Style::default().fg(accent_color)),
        Span::styled(empty_str, Style::default().fg(t.dimmed)),
        Span::raw("  "),
        Span::styled(right, Style::default().fg(t.dimmed)),
    ]);
    if area.height <= 1 {
        frame.render_widget(
            Paragraph::new(progress).style(style_with_bg(t.surface)),
            area,
        );
        return;
    }
    let lines = vec![Line::from(""), Line::from(""), progress, Line::from("")];
    frame.render_widget(Paragraph::new(lines).style(style_with_bg(t.surface)), area);
}

fn render_progress_widget(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;
    let accent_color = app.accent();

    if app.radio_buffering() {
        render_live_progress_line(
            app,
            frame,
            area,
            t,
            accent_color,
            "Buffering",
            String::new(),
            false,
        );
        return;
    }

    if app.radio_now_playing_active() && app.playback.current_song.is_some() {
        let e = app.playback.elapsed.as_secs();
        let elapsed_str = format!("{}:{:02}", e / 60, e % 60);
        render_live_progress_line(
            app,
            frame,
            area,
            t,
            accent_color,
            "● LIVE",
            elapsed_str,
            true,
        );
        return;
    }

    let (elapsed_str, total_str, ratio) = if app.playback.current_song.is_some() {
        let e = app.playback.elapsed.as_secs();
        let elapsed_str = format!("{}:{:02}", e / 60, e % 60);
        let (total_str, ratio) = match app.playback.total {
            Some(tot) => {
                let ts = tot.as_secs();
                let r = if ts > 0 {
                    (e as f64 / ts as f64).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (format!("{}:{:02}", ts / 60, ts % 60), r)
            }
            None => ("--:--".to_string(), 0.0),
        };
        (elapsed_str, total_str, ratio)
    } else {
        ("0:00".to_string(), "0:00".to_string(), 0.0)
    };

    let col_w = area.width as usize;
    let bar_w = col_w.saturating_sub(elapsed_str.len() + total_str.len() + 4);

    let spec = parse_progress_style(&app.config.progress_style);
    let (filled_str, partial_str, empty_str) = build_progress_bar(spec, ratio, bar_w);

    let progress = Line::from(vec![
        Span::styled(elapsed_str, Style::default().fg(t.dimmed)),
        Span::raw("  "),
        Span::styled(filled_str, Style::default().fg(accent_color)),
        Span::styled(partial_str, Style::default().fg(accent_color)),
        Span::styled(empty_str, Style::default().fg(t.dimmed)),
        Span::raw("  "),
        Span::styled(total_str, Style::default().fg(t.dimmed)),
    ]);

    if area.height <= 1 {
        frame.render_widget(
            Paragraph::new(progress).style(style_with_bg(t.surface)),
            area,
        );
        return;
    }

    let lines = vec![Line::from(""), Line::from(""), progress, Line::from("")];

    frame.render_widget(Paragraph::new(lines).style(style_with_bg(t.surface)), area);
}

#[cfg(test)]
mod progress_style_tests {
    use super::{build_progress_bar, parse_progress_style, ProgressStyleSpec};

    #[test]
    fn ncmpcpp_half() {
        let s = parse_progress_style("=>-");
        let (a, b, c) = build_progress_bar(s, 0.5, 10);
        assert_eq!(a, "=====");
        assert_eq!(b, ">");
        assert_eq!(c, "----");
    }

    #[test]
    fn ncmpcpp_full() {
        let s = parse_progress_style("=>-");
        let (a, b, c) = build_progress_bar(s, 1.0, 8);
        assert_eq!(a.len(), 8);
        assert!(b.is_empty());
        assert!(c.is_empty());
    }

    #[test]
    fn hidden_empty_string() {
        let s = parse_progress_style("");
        assert!(matches!(s, ProgressStyleSpec::Hidden));
        let (a, b, c) = build_progress_bar(s, 0.5, 10);
        assert!(a.is_empty() && b.is_empty() && c.is_empty());
    }

    #[test]
    fn default_blocks_duplicated_playhead_is_fractional() {
        let s = parse_progress_style("██░");
        assert!(matches!(
            s,
            ProgressStyleSpec::FractionalBlocks {
                filled: '█',
                empty: '░'
            }
        ));
    }

    #[test]
    fn invalid_length_falls_back_to_original_block_look() {
        let s = parse_progress_style("blocks");
        assert!(matches!(
            s,
            ProgressStyleSpec::FractionalBlocks {
                filled: '█',
                empty: '░'
            }
        ));
    }

    #[test]
    fn fractional_partial_cell() {
        let s = parse_progress_style("██░");
        let (a, b, c) = build_progress_bar(s, 0.02, 8);
        assert_eq!(a, "");
        assert_eq!(b, "▏");
        assert_eq!(c.chars().count(), 7);
    }
}

#[cfg(test)]
mod controls_click_tests {
    use super::{
        centered_line_x, control_segments, controls_click_action_for, controls_line_plain,
        controls_line_width, controls_row_rect, segment_center, ControlSlot, ControlsClickCtx,
    };
    use crate::action::Action;
    use ratatui::layout::Rect;

    fn playing() -> ControlsClickCtx {
        ControlsClickCtx {
            has_current_song: true,
            paused: false,
            shuffled: false,
        }
    }

    fn shuffled() -> ControlsClickCtx {
        ControlsClickCtx {
            has_current_song: true,
            paused: false,
            shuffled: true,
        }
    }

    fn line_start(area: Rect, ctx: ControlsClickCtx) -> u16 {
        centered_line_x(area.x, area.width, controls_line_width(ctx))
    }

    fn click_at_slot(ctx: ControlsClickCtx, area: Rect, slot: ControlSlot) -> Option<Action> {
        let (_, segments) = control_segments(ctx);
        let x = line_start(area, ctx) + segment_center(&segments, slot);
        controls_click_action_for(ctx, area, x)
    }

    #[test]
    fn line_start_matches_ratatui_center_formula() {
        let ctx = ControlsClickCtx {
            has_current_song: false,
            paused: false,
            shuffled: false,
        };
        let total = controls_line_width(ctx);
        let area = Rect::new(5, 0, 40, 1);
        let legacy = area.x + (area.width.saturating_sub(total)) / 2;
        let aligned = line_start(area, ctx);
        assert_eq!(aligned, area.x + (area.width / 2).saturating_sub(total / 2));
        assert_ne!(
            legacy, aligned,
            "regression: must not use (W-L)/2 centering"
        );
    }

    #[test]
    fn line_width_idle_vs_playing() {
        let idle = ControlsClickCtx {
            has_current_song: false,
            paused: false,
            shuffled: false,
        };
        assert_eq!(
            controls_line_width(idle),
            controls_line_plain(idle).width() as u16
        );
        assert!(controls_line_width(playing()) >= controls_line_width(idle));
    }

    #[test]
    fn row_rect_single_line_footer() {
        let area = Rect::new(10, 20, 40, 1);
        let row = controls_row_rect(area);
        assert_eq!(row, Rect::new(10, 20, 40, 1));
    }

    #[test]
    fn row_rect_multi_line_column() {
        let area = Rect::new(10, 20, 40, 4);
        let row = controls_row_rect(area);
        assert_eq!(row, Rect::new(10, 21, 40, 1));
    }

    #[test]
    fn centered_clicks_map_to_all_five_buttons() {
        let area = Rect::new(0, 0, 80, 3);
        let ctx = playing();

        assert!(matches!(
            click_at_slot(ctx, area, ControlSlot::Shuffle),
            Some(Action::Shuffle)
        ));
        assert!(matches!(
            click_at_slot(ctx, area, ControlSlot::Prev),
            Some(Action::PrevTrack)
        ));
        assert!(matches!(
            click_at_slot(ctx, area, ControlSlot::PlayPause),
            Some(Action::PlayPause)
        ));
        assert!(matches!(
            click_at_slot(ctx, area, ControlSlot::Next),
            Some(Action::NextTrack)
        ));
        assert!(matches!(
            click_at_slot(ctx, area, ControlSlot::Loop),
            Some(Action::ToggleQueueLoop)
        ));
    }

    #[test]
    fn shuffle_button_toggles_unshuffle_when_shuffled() {
        let area = Rect::new(0, 0, 80, 1);
        let ctx = shuffled();
        assert!(matches!(
            click_at_slot(ctx, area, ControlSlot::Shuffle),
            Some(Action::Unshuffle)
        ));
    }

    #[test]
    fn clicks_outside_centered_strip_are_ignored() {
        let area = Rect::new(0, 0, 80, 1);
        let ctx = playing();
        assert!(controls_click_action_for(ctx, area, 0).is_none());
        assert!(controls_click_action_for(ctx, area, 79).is_none());
    }

    #[test]
    fn gap_clicks_do_not_hit_buttons() {
        let area = Rect::new(0, 0, 80, 1);
        let ctx = playing();
        let (total, segments) = control_segments(ctx);
        let base = line_start(area, ctx);
        let gap_x = base + segments[0].start + segments[0].width + 3;
        assert!(gap_x < base + total);
        assert!(controls_click_action_for(ctx, area, gap_x).is_none());
    }
}
