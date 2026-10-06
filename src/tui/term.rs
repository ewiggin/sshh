//! Terminal handling: entering/leaving TUI mode, $EDITOR and clipboard.

use std::io::{Write, stdout};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Command, Stdio};
use std::{env, fs};

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub fn init() -> Result<DefaultTerminal> {
    // ratatui installs its own hook (restores the terminal and calls this one).
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture, Show);
        hook(info);
    }));
    let terminal = ratatui::try_init()?;
    execute!(stdout(), EnableMouseCapture)?;
    Ok(terminal)
}

/// Leaves TUI mode. Also shows the cursor explicitly: ratatui only does it when
/// the `Terminal` is dropped, which never happens if we `exec` ssh afterwards.
pub fn restore() {
    let _ = execute!(stdout(), DisableMouseCapture, Show);
    ratatui::restore();
}

/// Opens `text` in $VISUAL/$EDITOR (or vi) and returns the result. Suspends the
/// TUI meanwhile.
pub fn edit_external(terminal: &mut DefaultTerminal, text: &str) -> Result<String> {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    let path = env::temp_dir().join(format!("sshh-{}.md", std::process::id()));
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?
        .write_all(text.as_bytes())?;

    execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen, Show)?;
    disable_raw_mode()?;
    // The editor may carry arguments (`code --wait`), so it goes through the shell.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(&path)
        .status();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    terminal.clear()?;

    let result = match status {
        Ok(s) if s.success() => fs::read_to_string(&path).context("reading the edited file"),
        Ok(s) => Err(anyhow::anyhow!("the editor exited with {s}")),
        Err(e) => Err(anyhow::anyhow!(e).context(format!("running {editor}"))),
    };
    let _ = fs::remove_file(&path);
    result
}

/// Copies to the clipboard. Uses wl-copy/xclip when available; otherwise OSC 52
/// (supported by most terminals, and it also works over ssh).
pub fn copy(text: &str) -> Result<()> {
    let tool: Option<&[&str]> = if env::var_os("WAYLAND_DISPLAY").is_some() {
        Some(&["wl-copy"])
    } else if env::var_os("DISPLAY").is_some() {
        Some(&["xclip", "-selection", "clipboard"])
    } else {
        None
    };
    if let Some([cmd, args @ ..]) = tool
        && let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    {
        child.stdin.take().context("stdin")?.write_all(text.as_bytes())?;
        if child.wait()?.success() {
            return Ok(());
        }
        bail!("{cmd} failed");
    }
    let mut out = stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()?;
    Ok(())
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"ssh -p 22 a@b"), "c3NoIC1wIDIyIGFAYg==");
    }
}
