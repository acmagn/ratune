use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::{App, Tab};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Left,
    Right,
    Full,
}

pub fn placement_from_str(s: &str) -> Option<Placement> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("left") {
        return Some(Placement::Left);
    }
    if s.eq_ignore_ascii_case("right") {
        return Some(Placement::Right);
    }
    if s.eq_ignore_ascii_case("full") {
        return Some(Placement::Full);
    }
    None
}

pub fn side_from_placement(p: Placement) -> Option<Side> {
    match p {
        Placement::Left => Some(Side::Left),
        Placement::Right => Some(Side::Right),
        Placement::Full => None,
    }
}

/// Core Now Playing tab horizontal split: returns `(left_col, right_col)`.
///
/// - When `split` is false, right has full width and left is zero.
/// - When `split` is true, `left_width_percent` controls the left column width.
pub fn now_playing_split_lr(center: Rect, split: bool, left_width_percent: u8) -> (Rect, Rect) {
    if !split {
        return (Rect::new(center.x, center.y, 0, center.height), center);
    }
    let left_w = left_width_percent.clamp(1, 99);
    let right_w = 100u8.saturating_sub(left_w).max(1);
    let cols = Layout::horizontal([
        Constraint::Percentage(left_w.into()),
        Constraint::Percentage(right_w.into()),
    ])
    .split(center);
    (cols[0], cols[1])
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NowPlayingRects {
    pub art: Option<Rect>,
    pub queue: Option<Rect>,
    pub visualizer: Option<Rect>,
    pub now_playing: Option<Rect>,
    pub lyrics: Option<Rect>,
}

fn split_vertical_equal(area: Rect, parts: usize) -> Vec<Rect> {
    if parts == 0 {
        return vec![];
    }
    let pct = (100 / parts).max(1) as u16;
    let mut cs = Vec::with_capacity(parts);
    for _ in 0..parts {
        cs.push(Constraint::Percentage(pct));
    }
    Layout::default()
        .direction(Direction::Vertical)
        .constraints(cs)
        .split(area)
        .to_vec()
}

fn split_vertical_two_weighted(area: Rect, top_percent: u8) -> [Rect; 2] {
    let top = top_percent.clamp(1, 99);
    let bottom = 100u8.saturating_sub(top).max(1);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(top.into()),
            Constraint::Percentage(bottom.into()),
        ])
        .split(area);
    [rows[0], rows[1]]
}

fn split_main_and_docks(area: Rect, dock_count: usize) -> (Rect, Vec<Rect>) {
    match dock_count {
        0 => (area, vec![]),
        1 => {
            let rows = Layout::vertical([Constraint::Percentage(75), Constraint::Percentage(25)])
                .split(area);
            (rows[0], vec![rows[1]])
        }
        _ => {
            let rows = Layout::vertical([
                Constraint::Percentage(50),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(area);
            (rows[0], vec![rows[1], rows[2]])
        }
    }
}

/// Unified Now Playing tab rect computation for rendering + hit-testing + kitty art placement.
pub fn now_playing_rects(
    center: Rect,
    show_art: bool,
    art_position: Placement,
    queue_position: Placement,
    left_width_percent: u8,
    vertical_fill_top_percent: u8,
    visualizer_visible: bool,
    visualizer_position: Placement,
    lyrics_visible: bool,
    boxed_layout: bool,
    now_playing_position: Placement,
) -> NowPlayingRects {
    let art_pos = if show_art { Some(art_position) } else { None };
    let queue_pos = Some(queue_position);
    let vz_pos = if visualizer_visible {
        Some(visualizer_position)
    } else {
        None
    };
    let np_pos = if boxed_layout {
        Some(now_playing_position)
    } else {
        None
    };

    let art_side = art_pos.and_then(side_from_placement);
    let queue_side = queue_pos.and_then(side_from_placement);
    let vz_side = vz_pos.and_then(side_from_placement);
    let np_side = np_pos.and_then(side_from_placement);

    // "Full" means "span horizontally (full-width row)", not "take over everything".
    // We implement this by reserving full-width dock rows at the bottom for dock panes, and
    // full-width main rows at the top for art/queue when configured.
    let vz_full = vz_pos == Some(Placement::Full);
    let np_full = np_pos == Some(Placement::Full);
    let art_full = art_pos == Some(Placement::Full);
    let queue_full = queue_pos == Some(Placement::Full);

    // Full-width dock rows (visualizer / lyrics / now-playing).
    let dock_full_count = usize::from(vz_full)
        + usize::from(np_full)
        + usize::from(lyrics_visible && !visualizer_visible && queue_full);

    let (main_area, dock_full_rows) = split_main_and_docks(center, dock_full_count);

    let uses_left = queue_side == Some(Side::Left)
        || art_side == Some(Side::Left)
        || vz_side == Some(Side::Left)
        || np_side == Some(Side::Left);
    let uses_right = queue_side == Some(Side::Right)
        || art_side == Some(Side::Right)
        || vz_side == Some(Side::Right)
        || np_side == Some(Side::Right);
    let split = uses_left && uses_right;

    let mut rects = NowPlayingRects::default();

    // Assign full-width dock panes in a stable bottom-dock order.
    // Order: visualizer, lyrics, now-playing.
    let mut dock_iter = dock_full_rows.into_iter();
    if vz_full {
        rects.visualizer = dock_iter.next();
    }
    if lyrics_visible && !visualizer_visible && queue_full {
        rects.lyrics = dock_iter.next();
    }
    if np_full {
        rects.now_playing = dock_iter.next();
    }

    // Main area may also contain full-width rows (art/queue) above the column layout.
    let full_main_count = usize::from(art_full) + usize::from(queue_full);
    // Even when `show_art` is false (no `art_side`), we may still have left/right panes
    // (visualizer / now-playing) that need space below a full-width queue row.
    let has_column_main = (!art_full && art_side.is_some())
        || (!queue_full && queue_side.is_some())
        || vz_side.is_some() && !vz_full
        || np_side.is_some() && !np_full
        || (lyrics_visible && !visualizer_visible && queue_side.is_some());
    let top_parts = full_main_count + usize::from(has_column_main);
    let main_parts = split_vertical_equal(main_area, top_parts.max(1));
    let mut idx = 0usize;
    if art_full {
        rects.art = Some(main_parts[idx]);
        idx += 1;
    }
    if queue_full {
        rects.queue = Some(main_parts[idx]);
        idx += 1;
    }
    let column_area = if has_column_main {
        main_parts[idx]
    } else {
        // Nothing else; all main area already consumed.
        Rect::new(main_area.x, main_area.y, 0, 0)
    };

    let mut render_side = |side: Side, col: Rect| {
        let has_queue = queue_side == Some(side);
        let has_art = art_side == Some(side);
        let has_vz = vz_side == Some(side) && !vz_full;
        let has_np = np_side == Some(side) && !np_full;
        let has_lyrics = lyrics_visible && !visualizer_visible && has_queue;

        let docks = u16::from(has_vz) + u16::from(has_np) + u16::from(has_lyrics);

        // If there's exactly one thing on this side, let it fill vertically.
        let total_things = u16::from(has_queue) + u16::from(has_art) + docks;
        if total_things == 1 {
            if has_art {
                rects.art = Some(col);
            } else if has_queue {
                rects.queue = Some(col);
            } else if has_vz {
                rects.visualizer = Some(col);
            } else if has_np {
                rects.now_playing = Some(col);
            } else if has_lyrics {
                rects.lyrics = Some(col);
            }
            return;
        }

        // Exactly two things on this side: use weighted vertical split (configurable).
        if total_things == 2 {
            let [top, bottom] = split_vertical_two_weighted(col, vertical_fill_top_percent);
            let mut slots = [top, bottom].into_iter();

            if has_art {
                rects.art = slots.next();
            }
            if has_queue {
                rects.queue = slots.next();
            }
            if has_vz {
                rects.visualizer = slots.next();
            }
            if has_lyrics {
                rects.lyrics = slots.next();
            }
            if has_np {
                rects.now_playing = slots.next();
            }
            return;
        }

        // Multiple widgets on this side: stack them vertically with equal-ish shares.
        // Avoid nested 75/25 splits (they collapse badly when heights are small).
        let parts = usize::from(has_art)
            + usize::from(has_queue)
            + usize::from(has_vz)
            + usize::from(has_np)
            + usize::from(has_lyrics);
        let rows = split_vertical_equal(col, parts.max(1));
        let mut i = 0usize;

        if has_art {
            rects.art = Some(rows[i]);
            i += 1;
        }
        if has_queue {
            rects.queue = Some(rows[i]);
            i += 1;
        }
        if has_vz {
            rects.visualizer = Some(rows[i]);
            i += 1;
        }
        if has_lyrics {
            rects.lyrics = Some(rows[i]);
            i += 1;
        }
        if has_np {
            rects.now_playing = Some(rows[i]);
        }
    };

    if split {
        let (l, r) = now_playing_split_lr(column_area, true, left_width_percent);
        render_side(Side::Left, l);
        render_side(Side::Right, r);
    } else {
        // Single column: everything shares the full center.
        let anchor_side = art_side
            .or(vz_side)
            .or(np_side)
            .or(queue_side)
            .unwrap_or(Side::Right);
        render_side(anchor_side, column_area);
    }

    rects
}

/// Options for [`build_layout`]: tab strip position and now-playing bar height.
#[derive(Debug, Clone, Copy)]
pub struct LayoutOptions {
    /// When true: `tab_bar` is directly under the top edge; otherwise below `now_playing`.
    pub tab_bar_top: bool,
    /// Height of the now-playing bar in terminal rows (clamped when building).
    pub now_playing_bar_height: u16,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            tab_bar_top: false,
            now_playing_bar_height: 4,
        }
    }
}

/// Unified areas struct used by `build_layout` (all three tabs).
pub struct LayoutAreas {
    pub center: Rect,
    pub now_playing: Rect,
    /// Tab indicator bar — height 1.
    pub tab_bar: Rect,
    pub status_bar: Rect,
}

/// Unified layout for all tabs.
///
/// Default (`tab_bar_top` false): `center | now_playing | tab_bar | status_bar`.
/// With `tab_bar_top` true: `tab_bar | center | now_playing | status_bar`.
pub fn build_layout(area: Rect, opts: &LayoutOptions) -> LayoutAreas {
    let tab_h = 1u16;
    let status_h = 1u16;
    let np_h = opts
        .now_playing_bar_height
        .max(2)
        .min(area.height.saturating_sub(tab_h + status_h + 1));

    let min_center = area.height.saturating_sub(np_h + tab_h + status_h);
    let min_center = min_center.max(1);

    if opts.tab_bar_top {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(tab_h),
                Constraint::Min(min_center),
                Constraint::Length(np_h),
                Constraint::Length(status_h),
            ])
            .split(area);

        LayoutAreas {
            tab_bar: chunks[0],
            center: chunks[1],
            now_playing: chunks[2],
            status_bar: chunks[3],
        }
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(min_center),
                Constraint::Length(np_h),
                Constraint::Length(tab_h),
                Constraint::Length(status_h),
            ])
            .split(area);

        LayoutAreas {
            center: chunks[0],
            now_playing: chunks[1],
            tab_bar: chunks[2],
            status_bar: chunks[3],
        }
    }
}

/// Rows needed for boxed-mode footer (controls / progress outside the center pane), plus one blank
/// row between them when both are shown so they are not stacked flush.
fn boxed_np_footer_row_count(app: &App) -> u16 {
    let c_out =
        app.config.now_playing_show_controls && !app.config.now_playing_box_include_controls;
    let p_out =
        app.config.now_playing_show_progress && !app.config.now_playing_box_include_progress;
    let mut n = u16::from(c_out) + u16::from(p_out);
    if c_out && p_out {
        n += 1;
    }
    n
}

/// [`LayoutOptions`] for the current frame: when the Now Playing tab uses boxed layout, the bottom
/// strip only holds optional footer chrome — reserve matching height (not the full row-mode size).
pub fn layout_options_for_app(app: &App) -> LayoutOptions {
    let base = app.config.layout_options();
    if app
        .config
        .now_playing_layout
        .trim()
        .eq_ignore_ascii_case("boxed")
        && app.active_tab == Tab::NowPlaying
        && !app.lyrics_visible
    {
        let fh = boxed_np_footer_row_count(app);
        let need = if fh == 0 { 2u16 } else { fh.max(2) };
        // Shrink below [ui].now_playing_bar_height when the footer is smaller; grow if the footer
        // needs more rows than the config minimum.
        let h = if need > base.now_playing_bar_height {
            need
        } else {
            need.min(base.now_playing_bar_height)
        };
        return LayoutOptions {
            tab_bar_top: base.tab_bar_top,
            now_playing_bar_height: h,
        };
    }
    base
}

// (Old specialized Now Playing rect helpers removed: use `now_playing_rects` instead.)
