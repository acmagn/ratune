//! Run `fzf` (or `sk`) with stdin fed from the library index; used after
//! suspending the TUI (raw mode + alternate screen).

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use crossterm::event::{
    DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// Key name fzf prints when using `--expect=ctrl-r` and the user accepts with that key.
pub const FZF_EXPECT_REPLACE_QUEUE: &str = "ctrl-r";

/// Default fzf `--expect` key that flips songs ↔ albums (`[library.fzf].switch_key`).
pub const FZF_EXPECT_SWITCH_MODE_DEFAULT: &str = "ctrl-s";

/// Normalize an fzf `--expect` name from config. Empty → disabled. `+` is treated as `-`
/// so `ctrl+s` matches fzf's `ctrl-s`. `ctrl-r` is rejected (replace-queue).
pub fn normalize_fzf_expect_key(raw: &str) -> Option<String> {
    let key = raw.trim().to_ascii_lowercase().replace('+', "-");
    if key.is_empty() || key == FZF_EXPECT_REPLACE_QUEUE {
        return None;
    }
    Some(key)
}

/// What the library picker lists, one row per item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FzfPickerMode {
    Songs,
    Albums,
}

impl FzfPickerMode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Songs => Self::Albums,
            Self::Albums => Self::Songs,
        }
    }
}

/// How the user closed the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FzfAccept {
    Append,
    Replace,
    SwitchMode,
}

/// Split fzf stdout when `--expect` is set: first line is the key (empty string for default
/// Enter) or, without `--expect`, the first selected row.
pub fn parse_fzf_output_lines(
    lines: &[String],
    switch_key: Option<&str>,
) -> (FzfAccept, Vec<String>) {
    if lines.is_empty() {
        return (FzfAccept::Append, Vec::new());
    }
    let first = lines[0].as_str();
    if first == FZF_EXPECT_REPLACE_QUEUE {
        return (FzfAccept::Replace, lines[1..].to_vec());
    }
    if switch_key.is_some_and(|k| first.eq_ignore_ascii_case(k)) {
        return (FzfAccept::SwitchMode, Vec::new());
    }
    if first.is_empty() {
        return (FzfAccept::Append, lines[1..].to_vec());
    }
    (FzfAccept::Append, lines.to_vec())
}

/// True if `binary`'s file stem is `sk` (skim). Handles `sk.exe` on Windows.
pub fn fuzzy_picker_basename_is_sk(binary: &str) -> bool {
    let name = Path::new(binary)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(binary);
    name.strip_suffix(".exe").unwrap_or(name) == "sk"
}

fn bind_string_sets_key(bind_list: &str, key: &str) -> bool {
    bind_list.split(',').any(|frag| {
        let f = frag.trim();
        let Some((k, _)) = f.split_once(':') else {
            return false;
        };
        k.eq_ignore_ascii_case(key)
    })
}

/// True if argv already assigns an action to `key` via `--bind` / `-b`.
fn args_specify_bind(args: &[String], key: &str) -> bool {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(rest) = a.strip_prefix("--bind=").or_else(|| a.strip_prefix("-b=")) {
            if bind_string_sets_key(rest, key) {
                return true;
            }
        } else if (a == "--bind" || a == "-b") && i + 1 < args.len() {
            if bind_string_sets_key(&args[i + 1], key) {
                return true;
            }
            i += 1;
        }
        i += 1;
    }
    false
}

/// Add `key` to the first `--expect` list (or append `--expect=key`). A single list keeps
/// fzf and skim behaving the same.
fn add_expect_key(args: &mut Vec<String>, key: &str) {
    let has_key = |list: &str| list.split(',').any(|k| k.trim().eq_ignore_ascii_case(key));
    let mut i = 0;
    while i < args.len() {
        if let Some(list) = args[i].strip_prefix("--expect=") {
            if !has_key(list) {
                args[i] = format!("--expect={list},{key}");
            }
            return;
        }
        if args[i] == "--expect" && i + 1 < args.len() {
            if !has_key(&args[i + 1]) {
                args[i + 1] = format!("{},{key}", args[i + 1]);
            }
            return;
        }
        i += 1;
    }
    args.push(format!("--expect={key}"));
}

/// Add the songs ↔ albums switch key to `--expect` when `switch_key` is set. Skim's default
/// bindings (Ctrl+R rotates fuzzy/regex mode) win over `--expect`, so for `sk` also append
/// `--bind=<key>:accept(<key>)` for each picker key the user has not bound.
pub fn prepare_library_fuzzy_picker_args(
    binary: &str,
    mut args: Vec<String>,
    switch_key: Option<&str>,
) -> Vec<String> {
    if let Some(key) = switch_key {
        add_expect_key(&mut args, key);
    }
    if !fuzzy_picker_basename_is_sk(binary) {
        return args;
    }
    let mut bind_keys = vec![FZF_EXPECT_REPLACE_QUEUE];
    if let Some(key) = switch_key {
        bind_keys.push(key);
    }
    for key in bind_keys {
        if !args_specify_bind(&args, key) {
            args.push(format!("--bind={key}:accept({key})"));
        }
    }
    args
}

/// Suspend the TUI so a subprocess can use the terminal normally.
pub fn suspend_tui<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    in_tmux: bool,
) -> Result<()> {
    disable_raw_mode().context("disable_raw_mode")?;
    terminal.backend_mut().execute(DisableMouseCapture)?;
    if in_tmux {
        terminal
            .backend_mut()
            .write_all(b"\x1bPtmux;\x1b\x1b[?1004l\x1b\\")?;
        terminal.backend_mut().flush()?;
    } else {
        terminal.backend_mut().execute(DisableFocusChange)?;
    }
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    terminal.backend_mut().flush()?;
    // Snapshot the cooked mode `disable_raw_mode` just restored (usually IXON on).
    // We turn flow control off only for the picker; `resume_tui` puts this back
    // before `enable_raw_mode` so crossterm does not save IXON-off as "original".
    save_cooked_mode();
    disable_software_flow_control();
    Ok(())
}

/// After fzf exits, drop unread keypresses and keep software flow control off so
/// the next picker (or a held switch key) cannot XOFF the terminal.
pub fn prepare_picker_relaunch() {
    flush_pending_input();
    disable_software_flow_control();
}

#[cfg(unix)]
static COOKED_BEFORE_PICKER: std::sync::Mutex<Option<libc::termios>> = std::sync::Mutex::new(None);

#[cfg(unix)]
fn save_cooked_mode() {
    let mut term = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut term) } == 0 {
        if let Ok(mut guard) = COOKED_BEFORE_PICKER.lock() {
            *guard = Some(term);
        }
    }
}

#[cfg(not(unix))]
fn save_cooked_mode() {}

#[cfg(unix)]
fn restore_cooked_mode() {
    let Ok(mut guard) = COOKED_BEFORE_PICKER.lock() else {
        return;
    };
    if let Some(term) = guard.take() {
        unsafe {
            let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term);
        }
    }
}

#[cfg(not(unix))]
fn restore_cooked_mode() {}

#[cfg(unix)]
fn disable_software_flow_control() {
    unsafe {
        let mut term = std::mem::zeroed::<libc::termios>();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut term) != 0 {
            return;
        }
        term.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
        let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term);
    }
}

#[cfg(not(unix))]
fn disable_software_flow_control() {}

#[cfg(unix)]
fn flush_pending_input() {
    unsafe {
        let _ = libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
}

#[cfg(not(unix))]
fn flush_pending_input() {}

/// Restore raw mode + alternate screen after a subprocess exits.
pub fn resume_tui<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    in_tmux: bool,
) -> Result<()> {
    restore_cooked_mode();
    enable_raw_mode().context("enable_raw_mode")?;
    terminal.backend_mut().execute(EnterAlternateScreen)?;
    terminal.backend_mut().execute(EnableMouseCapture)?;
    if in_tmux {
        terminal
            .backend_mut()
            .write_all(b"\x1bPtmux;\x1b\x1b[?1004h\x1b\\")?;
        terminal.backend_mut().flush()?;
    } else {
        terminal.backend_mut().execute(EnableFocusChange)?;
    }
    terminal.hide_cursor()?;
    terminal.backend_mut().flush()?;
    Ok(())
}

/// Stream the lines produced by `write_input` to `binary` with `args`. With `--multi`, fzf
/// prints one selected line per row. Returns `None` on cancel / empty / non-zero exit (e.g. 130).
pub fn run_fzf(
    binary: &str,
    args: &[String],
    write_input: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
) -> Result<Option<Vec<String>>> {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to spawn {binary}"))?;

    if let Some(stdin) = child.stdin.take() {
        let mut stdin = std::io::BufWriter::new(stdin);
        let written = write_input(&mut stdin).and_then(|()| stdin.flush());
        match written {
            // The picker can be accepted / switched / cancelled before it has read all input.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            other => other?,
        }
    }

    let mut stdout = child.stdout.take().context("fzf stdout")?;
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

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn parse(lines: &[String]) -> (FzfAccept, Vec<String>) {
        parse_fzf_output_lines(lines, Some(FZF_EXPECT_SWITCH_MODE_DEFAULT))
    }

    fn prepare(binary: &str, args: Vec<String>) -> Vec<String> {
        prepare_library_fuzzy_picker_args(binary, args, Some(FZF_EXPECT_SWITCH_MODE_DEFAULT))
    }

    #[test]
    fn parse_expect_enter_then_rows() {
        let lines = vec![String::new(), "id1\ta\tb".into(), "id2\ta\tb".into()];
        let (accept, rows) = parse(&lines);
        assert_eq!(accept, FzfAccept::Append);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn parse_expect_ctrl_r() {
        let lines = vec!["ctrl-r".into(), "id1\tx".into()];
        let (accept, rows) = parse(&lines);
        assert_eq!(accept, FzfAccept::Replace);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn parse_expect_switch_mode_drops_rows() {
        let lines = vec!["ctrl-s".into(), "id1\tx".into()];
        let (accept, rows) = parse(&lines);
        assert_eq!(accept, FzfAccept::SwitchMode);
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_no_expect_plain_rows() {
        let lines = vec!["id1\tx".into(), "id2\ty".into()];
        let (accept, rows) = parse(&lines);
        assert_eq!(accept, FzfAccept::Append);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn parse_custom_switch_key() {
        let lines = vec!["alt-a".into(), "id1\tx".into()];
        let (accept, rows) = parse_fzf_output_lines(&lines, Some("alt-a"));
        assert_eq!(accept, FzfAccept::SwitchMode);
        assert!(rows.is_empty());
        let (accept, _) = parse_fzf_output_lines(&lines, None);
        assert_eq!(accept, FzfAccept::Append);
    }

    #[test]
    fn normalize_switch_key() {
        assert_eq!(
            normalize_fzf_expect_key(" Ctrl+S ").as_deref(),
            Some("ctrl-s")
        );
        assert_eq!(normalize_fzf_expect_key("").as_deref(), None);
        assert_eq!(normalize_fzf_expect_key("  ").as_deref(), None);
        assert_eq!(normalize_fzf_expect_key("ctrl-r").as_deref(), None);
        assert_eq!(normalize_fzf_expect_key("alt-a").as_deref(), Some("alt-a"));
    }

    #[test]
    fn basename_is_sk() {
        assert!(fuzzy_picker_basename_is_sk("sk"));
        assert!(fuzzy_picker_basename_is_sk("/usr/bin/sk"));
        assert!(fuzzy_picker_basename_is_sk("sk.exe"));
        assert!(!fuzzy_picker_basename_is_sk("fzf"));
        assert!(!fuzzy_picker_basename_is_sk("/usr/bin/fzf"));
    }

    #[test]
    fn prepare_merges_switch_key_into_expect() {
        let out = prepare("fzf", strs(&["--expect=ctrl-r", "--multi"]));
        assert_eq!(out, strs(&["--expect=ctrl-r,ctrl-s", "--multi"]));
        let out = prepare("fzf", strs(&["--expect", "ctrl-r"]));
        assert_eq!(out, strs(&["--expect", "ctrl-r,ctrl-s"]));
        let out = prepare("fzf", strs(&["--expect=ctrl-s"]));
        assert_eq!(out, strs(&["--expect=ctrl-s"]));
        let out = prepare("fzf", strs(&["--multi"]));
        assert_eq!(out, strs(&["--multi", "--expect=ctrl-s"]));
        let out = prepare_library_fuzzy_picker_args("fzf", strs(&["--multi"]), None);
        assert_eq!(out, strs(&["--multi"]));
        let out =
            prepare_library_fuzzy_picker_args("fzf", strs(&["--expect=ctrl-r"]), Some("alt-a"));
        assert_eq!(out, strs(&["--expect=ctrl-r,alt-a"]));
    }

    #[test]
    fn prepare_sk_appends_accept_binds() {
        let out = prepare("sk", strs(&["--multi"]));
        assert_eq!(
            out,
            strs(&[
                "--multi",
                "--expect=ctrl-s",
                "--bind=ctrl-r:accept(ctrl-r)",
                "--bind=ctrl-s:accept(ctrl-s)",
            ])
        );
        let out = prepare("/x/sk", strs(&["--multi"]));
        assert_eq!(out.last().unwrap(), "--bind=ctrl-s:accept(ctrl-s)");
    }

    #[test]
    fn prepare_sk_skips_keys_already_bound() {
        let out = prepare("sk", strs(&["--bind=ctrl-r:ignore", "--expect=ctrl-r"]));
        assert_eq!(
            out,
            strs(&[
                "--bind=ctrl-r:ignore",
                "--expect=ctrl-r,ctrl-s",
                "--bind=ctrl-s:accept(ctrl-s)",
            ])
        );
        let out = prepare(
            "sk",
            strs(&["--bind", "ctrl-r:accept(ctrl-alt-m),ctrl-s:ignore"]),
        );
        assert_eq!(out.len(), 3);
    }
}
