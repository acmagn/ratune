//! Run `fzf` (or `sk`) with stdin fed from the library index; used after
//! suspending the TUI (raw mode + alternate screen).

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use crossterm::ExecutableCommand;
use crossterm::event::{
    DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Key name fzf prints when using `--expect=ctrl-r` and the user accepts with that key.
pub const FZF_EXPECT_REPLACE_QUEUE: &str = "ctrl-r";

/// Split fzf stdout when `--expect` is set: first line is the key (empty string for default
/// Enter) or, without `--expect`, the first selected row.
pub fn parse_fzf_output_lines(lines: &[String]) -> (bool, Vec<String>) {
    if lines.is_empty() {
        return (false, Vec::new());
    }
    let first = lines[0].as_str();
    if first == FZF_EXPECT_REPLACE_QUEUE {
        return (true, lines[1..].to_vec());
    }
    if first.is_empty() {
        return (false, lines[1..].to_vec());
    }
    (false, lines.to_vec())
}

/// Suspend the TUI so a subprocess can use the terminal normally.
pub fn suspend_tui<W: Write>(terminal: &mut Terminal<CrosstermBackend<W>>, in_tmux: bool) -> Result<()> {
    disable_raw_mode().context("disable_raw_mode")?;
    terminal.backend_mut().execute(DisableMouseCapture)?;
    if in_tmux {
        terminal.backend_mut().write_all(b"\x1bPtmux;\x1b\x1b[?1004l\x1b\\")?;
        terminal.backend_mut().flush()?;
    } else {
        terminal.backend_mut().execute(DisableFocusChange)?;
    }
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    terminal.backend_mut().flush()?;
    Ok(())
}

/// Restore raw mode + alternate screen after a subprocess exits.
pub fn resume_tui<W: Write>(terminal: &mut Terminal<CrosstermBackend<W>>, in_tmux: bool) -> Result<()> {
    enable_raw_mode().context("enable_raw_mode")?;
    terminal.backend_mut().execute(EnterAlternateScreen)?;
    terminal.backend_mut().execute(EnableMouseCapture)?;
    if in_tmux {
        terminal.backend_mut().write_all(b"\x1bPtmux;\x1b\x1b[?1004h\x1b\\")?;
        terminal.backend_mut().flush()?;
    } else {
        terminal.backend_mut().execute(EnableFocusChange)?;
    }
    terminal.hide_cursor()?;
    terminal.backend_mut().flush()?;
    Ok(())
}

/// Pipe `input` to `binary` with `args`. With `--multi`, fzf prints one selected
/// line per row. Returns `None` on cancel / empty / non-zero exit (e.g. 130).
pub fn run_fzf(binary: &str, args: &[String], input: &str) -> Result<Option<Vec<String>>> {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to spawn {binary}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }

    let mut stdout = child
        .stdout
        .take()
        .context("fzf stdout")?;
    let status = child.wait().context("wait fzf")?;

    let mut out = String::new();
    stdout.read_to_string(&mut out)?;

    if !status.success() {
        return Ok(None);
    }
    let lines: Vec<String> = out
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        Ok(None)
    } else {
        Ok(Some(lines))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_expect_enter_then_rows() {
        let lines = vec![
            String::new(),
            "id1\ta\tb".into(),
            "id2\ta\tb".into(),
        ];
        let (replace, rows) = parse_fzf_output_lines(&lines);
        assert!(!replace);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn parse_expect_ctrl_r() {
        let lines = vec![
            "ctrl-r".into(),
            "id1\tx".into(),
        ];
        let (replace, rows) = parse_fzf_output_lines(&lines);
        assert!(replace);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn parse_no_expect_plain_rows() {
        let lines = vec!["id1\tx".into(), "id2\ty".into()];
        let (replace, rows) = parse_fzf_output_lines(&lines);
        assert!(!replace);
        assert_eq!(rows.len(), 2);
    }
}
