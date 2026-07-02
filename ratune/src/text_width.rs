//! Terminal display width (wcwidth-style), for column alignment in monospace UIs.
//!
//! CJK and many other scripts occupy two columns; Rust's `format!("{:<n$}")` counts
//! Unicode scalars instead, which misaligns padded columns.

use unicode_width::UnicodeWidthChar;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Sum of terminal column widths for printable characters in `s`.
pub fn str_width(s: &str) -> usize {
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// Truncate `s` to at most `max_cols` terminal columns, appending `…` when cut.
pub fn truncate_to_width(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if str_width(s) <= max_cols {
        return s.to_string();
    }
    if max_cols == 1 {
        return "…".to_string();
    }
    let budget = max_cols - 1;
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            out.push('…');
            return out;
        }
        used += w;
        out.push(ch);
    }
    out
}

/// Pad `s` to exactly `cols` terminal columns (no truncation).
pub fn pad_to_width(s: &str, cols: usize, align: Align) -> String {
    if cols == 0 {
        return String::new();
    }
    let w = str_width(s);
    if w >= cols {
        return s.to_string();
    }
    let pad = cols - w;
    match align {
        Align::Left => format!("{s}{}", " ".repeat(pad)),
        Align::Right => format!("{}{s}", " ".repeat(pad)),
    }
}

/// Truncate then pad so the result occupies exactly `cols` terminal columns.
pub fn fit_to_width(s: &str, cols: usize, align: Align) -> String {
    if cols == 0 {
        return String::new();
    }
    pad_to_width(&truncate_to_width(s, cols), cols, align)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_width_matches_len() {
        assert_eq!(str_width("hello"), 5);
        assert_eq!(fit_to_width("hi", 5, Align::Left), "hi   ");
        assert_eq!(fit_to_width("hi", 5, Align::Right), "   hi");
    }

    #[test]
    fn cjk_counts_as_two_columns() {
        assert_eq!(str_width("日本"), 4);
        let padded = fit_to_width("日本", 6, Align::Left);
        assert_eq!(str_width(&padded), 6);
        assert_eq!(padded, "日本  ");
        let truncated = fit_to_width("日本語の歌", 6, Align::Left);
        assert_eq!(str_width(&truncated), 6);
        assert_eq!(truncated, "日本… ");
    }

    #[test]
    fn mixed_ascii_and_cjk_aligns() {
        let title = fit_to_width("Hello 世界", 12, Align::Left);
        assert_eq!(str_width(&title), 12);
        let artist = fit_to_width("Artist", 8, Align::Left);
        assert_eq!(str_width(&artist), 8);
    }

    #[test]
    fn truncate_reserves_ellipsis_column() {
        assert_eq!(truncate_to_width("abcdef", 4), "abc…");
        assert_eq!(str_width(&truncate_to_width("abcdef", 4)), 4);
    }
}
