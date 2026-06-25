//! Shared list scroll helpers for overlay panes.

use ratatui::layout::Rect;
use ratatui::widgets::ListState;

use crate::state::LibraryState;

pub fn list_state_for_selection(
    area: Rect,
    sel: Option<usize>,
    len: usize,
    scroll: &mut usize,
) -> ListState {
    let vh = area.height.saturating_sub(2).max(1) as usize;
    let mut state = ListState::default();
    if len == 0 {
        *scroll = 0;
        return state;
    }
    if let Some(sel) = sel {
        if sel < len {
            LibraryState::clamp_vertical_scroll(scroll, sel, len, vh);
            state = ListState::default().with_offset(*scroll);
            state.select(Some(sel));
        }
    } else {
        let max_first = len.saturating_sub(vh);
        *scroll = (*scroll).min(max_first);
        state = ListState::default().with_offset(*scroll);
    }
    state
}

/// Map a click inside a bordered list column to an absolute row index.
pub fn list_index_at_click(area: Rect, x: u16, y: u16, scroll: usize, len: usize) -> Option<usize> {
    if x < area.x || x >= area.x + area.width {
        return None;
    }
    if y <= area.y || y >= area.y + area.height.saturating_sub(1) {
        return None;
    }
    if len == 0 {
        return None;
    }
    let visible_row = (y - area.y - 1) as usize;
    let idx = scroll + visible_row;
    (idx < len).then_some(idx)
}
