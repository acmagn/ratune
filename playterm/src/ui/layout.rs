use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::{App, Tab};

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
    pub center:     Rect,
    pub now_playing: Rect,
    /// Tab indicator bar — height 1.
    pub tab_bar:    Rect,
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
            tab_bar:     chunks[0],
            center:      chunks[1],
            now_playing: chunks[2],
            status_bar:  chunks[3],
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
            center:      chunks[0],
            now_playing: chunks[1],
            tab_bar:     chunks[2],
            status_bar:  chunks[3],
        }
    }
}

/// Rows needed for boxed-mode footer (controls / progress outside the center pane), plus one blank
/// row between them when both are shown so they are not stacked flush.
fn boxed_np_footer_row_count(app: &App) -> u16 {
    let c_out = app.config.now_playing_show_controls && !app.config.now_playing_box_include_controls;
    let p_out = app.config.now_playing_show_progress && !app.config.now_playing_box_include_progress;
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
        let need = if fh == 0 {
            2u16
        } else {
            fh.max(2)
        };
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

/// Split the Now Playing tab **center** into `(art column, queue column)`.
/// When `show_art` is false, art has zero width and the queue uses the full center.
pub fn now_playing_split_columns(
    center: Rect,
    show_art: bool,
    art_width_percent: u8,
    art_position_right: bool,
) -> (Rect, Rect) {
    if !show_art {
        return (Rect::new(center.x, center.y, 0, center.height), center);
    }

    let art_w = art_width_percent.clamp(1, 99);
    let queue_w = 100u8.saturating_sub(art_w).max(1);
    let cols = Layout::horizontal([
        Constraint::Percentage(art_w.into()),
        Constraint::Percentage(queue_w.into()),
    ])
    .split(center);

    if art_position_right {
        (cols[1], cols[0])
    } else {
        (cols[0], cols[1])
    }
}

/// Bottom dock rect for boxed now playing (same vertical share as Visualizer: 25% of a column).
///
/// `viz_under_art` — visualizer is visible **and** `[ui].visualizer_location` is `art` **and** `show_art`.
/// `np_under_art` — layout is boxed **and** `[ui].now_playing_box_location` is `art` **and** `show_art`.
/// `None` when not using boxed layout, or when lyrics hide the center pane.
pub fn now_playing_boxed_pane_rect(
    center: Rect,
    show_art: bool,
    art_width_percent: u8,
    art_position_right: bool,
    lyrics_visible: bool,
    visualizer_visible: bool,
    viz_under_art: bool,
    boxed_layout: bool,
    np_under_art: bool,
) -> Option<Rect> {
    if !boxed_layout || lyrics_visible {
        return None;
    }

    let (art_col, queue_col) =
        now_playing_split_columns(center, show_art, art_width_percent, art_position_right);

    if visualizer_visible && boxed_layout {
        if viz_under_art && np_under_art {
            if art_col.width == 0 {
                return None;
            }
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(50),
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                ])
                .split(art_col);
            return Some(rows[2]);
        }
        if viz_under_art && !np_under_art {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(75),
                    Constraint::Percentage(25),
                ])
                .split(queue_col);
            return Some(rows[1]);
        }
        if !viz_under_art && np_under_art {
            if art_col.width == 0 {
                return None;
            }
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(75),
                    Constraint::Percentage(25),
                ])
                .split(art_col);
            return Some(rows[1]);
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(queue_col);
        return Some(rows[2]);
    }

    if np_under_art {
        if art_col.width == 0 {
            return None;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(75),
                Constraint::Percentage(25),
            ])
            .split(art_col);
        return Some(rows[1]);
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(75),
            Constraint::Percentage(25),
        ])
        .split(queue_col);
    Some(rows[1])
}

/// Queue list area on the Now Playing tab: matches `nowplaying_tab::render`.
pub fn now_playing_queue_widget_rect(
    center: Rect,
    show_art: bool,
    art_width_percent: u8,
    art_position_right: bool,
    lyrics_visible: bool,
    visualizer_visible: bool,
    viz_under_art: bool,
    boxed_layout: bool,
    np_under_art: bool,
) -> Rect {
    let (_, queue_col) =
        now_playing_split_columns(center, show_art, art_width_percent, art_position_right);

    if lyrics_visible {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(75),
                Constraint::Percentage(25),
            ])
            .split(queue_col);
        return rows[0];
    }

    if visualizer_visible && boxed_layout {
        if viz_under_art && np_under_art {
            return queue_col;
        }
        if viz_under_art && !np_under_art {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(75),
                    Constraint::Percentage(25),
                ])
                .split(queue_col);
            return rows[0];
        }
        if !viz_under_art && np_under_art {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(75),
                    Constraint::Percentage(25),
                ])
                .split(queue_col);
            return rows[0];
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(queue_col);
        return rows[0];
    }

    if visualizer_visible && !boxed_layout {
        if !viz_under_art {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(75),
                    Constraint::Percentage(25),
                ])
                .split(queue_col);
            return rows[0];
        }
        return queue_col;
    }

    if boxed_layout && !visualizer_visible {
        if np_under_art {
            return queue_col;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(75),
                Constraint::Percentage(25),
            ])
            .split(queue_col);
        return rows[0];
    }

    queue_col
}

/// Kitty album-art placement: top segment of the art column only, so the image never covers
/// the visualizer or boxed now-playing dock below it.
pub fn now_playing_album_art_rect(
    terminal_size: Rect,
    layout_opts: &LayoutOptions,
    show_art: bool,
    art_width_percent: u8,
    art_position_right: bool,
    visualizer_visible: bool,
    viz_under_art: bool,
    boxed_layout: bool,
    np_under_art: bool,
) -> Option<Rect> {
    if !show_art {
        return None;
    }

    let areas = build_layout(terminal_size, layout_opts);
    let center = areas.center;
    let (art_col, _) = now_playing_split_columns(
        center,
        show_art,
        art_width_percent,
        art_position_right,
    );

    if art_col.width == 0 {
        return None;
    }

    if boxed_layout && np_under_art && visualizer_visible && viz_under_art {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(art_col);
        return Some(rows[0]);
    }

    if visualizer_visible && viz_under_art {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(75),
                Constraint::Percentage(25),
            ])
            .split(art_col);
        return Some(rows[0]);
    }

    if boxed_layout && np_under_art {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(75),
                Constraint::Percentage(25),
            ])
            .split(art_col);
        return Some(rows[0]);
    }

    Some(art_col)
}
