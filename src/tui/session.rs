//! Embedded ssh sessions: the system `ssh` running inside a pseudo terminal,
//! with its output fed into a VT100 emulator that the TUI renders.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use anyhow::{Context, Result, anyhow};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Lines of scrollback kept per session.
const SCROLLBACK: usize = 5000;

pub struct Session {
    parser: Arc<Mutex<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: (u16, u16),
    /// Exit description once the ssh process has finished.
    exit: Option<String>,
}

impl Session {
    /// Starts `program args` in a new pseudo terminal of `rows`x`cols`.
    /// `dirty` is set whenever new output arrives, so the TUI redraws.
    pub fn spawn(
        program: &Path,
        args: &[String],
        rows: u16,
        cols: u16,
        dirty: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (rows, cols) = (rows.max(2), cols.max(2));
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| anyhow!("{e:#}"))
            .context("opening a pseudo terminal")?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow!("{e:#}"))
            .with_context(|| format!("running {}", program.display()))?;
        // Only the child needs the slave side; dropping ours lets reads end at exit.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| anyhow!("{e:#}"))?;
        let writer = pair.master.take_writer().map_err(|e| anyhow!("{e:#}"))?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));

        let output = Arc::clone(&parser);
        thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut parser) = output.lock() {
                            parser.process(&buf[..n]);
                        }
                        dirty.store(true, Ordering::Release);
                    }
                }
            }
            dirty.store(true, Ordering::Release);
        });

        Ok(Self { parser, master: pair.master, writer, child, size: (rows, cols), exit: None })
    }

    pub fn parser(&self) -> MutexGuard<'_, vt100::Parser> {
        // A poisoned lock only means the reader thread panicked mid-update;
        // the screen is still usable.
        self.parser.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn exit(&self) -> Option<&str> {
        self.exit.as_deref()
    }

    /// Checks whether ssh has finished. Returns true if it just did.
    pub fn poll_exit(&mut self) -> bool {
        if self.exit.is_some() {
            return false;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.exit = Some(if status.success() {
                    "session ended".into()
                } else {
                    format!("session ended with code {}", status.exit_code())
                });
                true
            }
            Ok(None) => false,
            Err(e) => {
                self.exit = Some(format!("session lost: {e}"));
                true
            }
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if self.exit.is_none() {
            // A failed write means ssh is gone; poll_exit will report it.
            let _ = self.writer.write_all(bytes).and_then(|()| self.writer.flush());
        }
    }

    pub fn send_key(&mut self, key: KeyEvent) {
        let app_cursor = self.parser().screen().application_cursor();
        if let Some(bytes) = key_bytes(key, app_cursor) {
            self.parser().screen_mut().set_scrollback(0);
            self.write(&bytes);
        }
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self.parser().screen().bracketed_paste();
        let mut bytes = Vec::with_capacity(text.len() + 12);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.write(&bytes);
    }

    /// Scrolls the view back (positive) or forward (negative) through history.
    pub fn scroll(&mut self, delta: isize) {
        let mut parser = self.parser();
        let current = parser.screen().scrollback() as isize;
        parser.screen_mut().set_scrollback((current + delta).max(0) as usize);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(2), cols.max(2));
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        self.parser().screen_mut().set_size(rows, cols);
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.exit.is_none() {
            let _ = self.child.kill();
        }
    }
}

/// Bytes an xterm-compatible terminal sends for `key`.
pub fn key_bytes(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);

    let cursor = |letter: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[{n};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let with_alt = |mut bytes: Vec<u8>| {
        if alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };

    let bytes = match key.code {
        KeyCode::Char(c) if ctrl => with_alt(vec![control_byte(c)?]),
        KeyCode::Char(c) => with_alt(c.to_string().into_bytes()),
        KeyCode::Enter => with_alt(vec![b'\r']),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => with_alt(vec![if ctrl { 0x08 } else { 0x7f }]),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => return None,
    };
    Some(bytes)
}

/// Control character for Ctrl-`c`.
fn control_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '/' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn bytes(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        key_bytes(key(code, modifiers), false).unwrap()
    }

    #[test]
    fn plain_and_control_keys() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(bytes(KeyCode::Char('a'), none), b"a");
        assert_eq!(bytes(KeyCode::Char('é'), none), "é".as_bytes());
        assert_eq!(bytes(KeyCode::Char('c'), ctrl), [3]);
        assert_eq!(bytes(KeyCode::Char('w'), ctrl), [0x17]);
        assert_eq!(bytes(KeyCode::Char('d'), ctrl | KeyModifiers::SHIFT), [4]);
        assert_eq!(bytes(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb");
        assert_eq!(bytes(KeyCode::Enter, none), b"\r");
        assert_eq!(bytes(KeyCode::Backspace, none), [0x7f]);
        assert_eq!(bytes(KeyCode::Esc, none), [0x1b]);
        assert_eq!(bytes(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
    }

    #[test]
    fn cursor_and_function_keys() {
        let none = KeyModifiers::NONE;
        assert_eq!(bytes(KeyCode::Up, none), b"\x1b[A");
        assert_eq!(key_bytes(key(KeyCode::Up, none), true).unwrap(), b"\x1bOA");
        assert_eq!(bytes(KeyCode::Left, KeyModifiers::CONTROL), b"\x1b[1;5D");
        assert_eq!(bytes(KeyCode::Home, none), b"\x1b[H");
        assert_eq!(bytes(KeyCode::Delete, none), b"\x1b[3~");
        assert_eq!(bytes(KeyCode::PageDown, KeyModifiers::SHIFT), b"\x1b[6;2~");
        assert_eq!(bytes(KeyCode::F(1), none), b"\x1bOP");
        assert_eq!(bytes(KeyCode::F(5), none), b"\x1b[15~");
        assert_eq!(bytes(KeyCode::F(12), none), b"\x1b[24~");
    }

    #[test]
    fn session_runs_a_program() {
        let dirty = Arc::new(AtomicBool::new(false));
        let mut session = Session::spawn(
            Path::new("/bin/sh"),
            &["-c".into(), "read x; echo got:$x".into()],
            10,
            40,
            Arc::clone(&dirty),
        )
        .unwrap();
        session.write(b"hello\r");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.parser().screen().contents().contains("got:hello") {
            assert!(std::time::Instant::now() < deadline, "{}", session.parser().screen().contents());
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(dirty.load(Ordering::Acquire));
        while !session.poll_exit() {
            assert!(std::time::Instant::now() < deadline, "the program did not exit");
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(session.exit(), Some("session ended"));
        session.resize(20, 80);
        assert_eq!(session.parser().screen().size(), (20, 80));
    }
}
