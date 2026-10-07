//! Embedded ssh sessions: the system `ssh` running inside a pseudo terminal,
//! with its output fed into a VT100 emulator that the TUI renders.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use anyhow::{Context, Result, anyhow};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// Lines of scrollback kept per session.
const SCROLLBACK: usize = 5000;

/// The emulated terminal of a session.
pub type Terminal = vt100::Parser<Responder>;
type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

pub struct Session {
    parser: Arc<Mutex<Terminal>>,
    master: Box<dyn MasterPty + Send>,
    writer: Writer,
    child: Box<dyn Child + Send + Sync>,
    size: (u16, u16),
    /// Exit description once the ssh process has finished.
    exit: Option<String>,
    copy: Option<CopyMode>,
}

/// History mode (like tmux's copy mode): move through the session's history
/// with the keyboard, search it and copy lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyMode {
    /// Snapshot of every line (history + screen) when the mode started.
    lines: Vec<String>,
    /// Lines above the screen in that snapshot.
    history: usize,
    /// Highlighted line, as an index into `lines`.
    pub cursor: usize,
    /// Start of the line selection (`v`).
    pub anchor: Option<usize>,
    /// Search being typed after `/`.
    pub prompt: Option<String>,
    query: String,
    pending_g: bool,
    pub message: Option<String>,
}

impl CopyMode {
    pub fn total(&self) -> usize {
        self.lines.len()
    }

    /// Index in `lines` of the first screen row when scrolled back `offset`.
    pub fn top(&self, offset: usize) -> usize {
        self.history.saturating_sub(offset)
    }

    /// Lines highlighted: the selection, or just the cursor line.
    pub fn selected(&self) -> (usize, usize) {
        let anchor = self.anchor.unwrap_or(self.cursor);
        (anchor.min(self.cursor), anchor.max(self.cursor))
    }

    fn selected_text(&self) -> String {
        let (from, to) = self.selected();
        self.lines[from..=to].join("\n")
    }

    /// Next line (older if `backwards`) containing the query, wrapping around.
    fn find(&self, backwards: bool) -> Option<usize> {
        let query = self.query.to_lowercase();
        let n = self.lines.len();
        (1..=n)
            .map(|step| if backwards { (self.cursor + n - step) % n } else { (self.cursor + step) % n })
            .find(|&i| self.lines[i].to_lowercase().contains(&query))
    }
}

/// What a key did in history mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyAction {
    None,
    Exit,
    Copy(String),
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
        let writer: Writer =
            Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| anyhow!("{e:#}"))?));
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK,
            Responder::default(),
        )));

        let output = Arc::clone(&parser);
        let replies = Arc::clone(&writer);
        thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let answer = match output.lock() {
                            Ok(mut parser) => {
                                parser.process(&buf[..n]);
                                parser.callbacks_mut().take()
                            }
                            Err(_) => Vec::new(),
                        };
                        // Programs wait for these answers, so send them right away.
                        if !answer.is_empty()
                            && let Ok(mut writer) = replies.lock()
                        {
                            let _ = writer.write_all(&answer).and_then(|()| writer.flush());
                        }
                        dirty.store(true, Ordering::Release);
                    }
                }
            }
            dirty.store(true, Ordering::Release);
        });

        Ok(Self {
            parser,
            master: pair.master,
            writer,
            child,
            size: (rows, cols),
            exit: None,
            copy: None,
        })
    }

    pub fn parser(&self) -> MutexGuard<'_, Terminal> {
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
            let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            let _ = writer.write_all(bytes).and_then(|()| writer.flush());
        }
    }

    pub fn send_key(&mut self, key: KeyEvent) {
        let app_cursor = self.parser().screen().application_cursor();
        if let Some(bytes) = key_bytes(key, app_cursor) {
            self.parser().screen_mut().set_scrollback(0);
            self.write(&bytes);
        }
    }

    /// Whether the program in the session asked for mouse events.
    pub fn wants_mouse(&self) -> bool {
        self.parser().screen().mouse_protocol_mode() != MouseProtocolMode::None
    }

    /// Sends a mouse event, with `column`/`row` relative to the session screen.
    pub fn send_mouse(&mut self, event: MouseEvent) {
        let (mode, encoding) = {
            let parser = self.parser();
            let screen = parser.screen();
            (screen.mouse_protocol_mode(), screen.mouse_protocol_encoding())
        };
        if let Some(bytes) = mouse_bytes(event, mode, encoding) {
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

    pub fn copy_mode(&self) -> Option<&CopyMode> {
        self.copy.as_ref()
    }

    /// Enters history mode on the last line shown (or the cursor line).
    pub fn start_copy_mode(&mut self) {
        let copy = {
            let mut parser = self.parser();
            let screen = parser.screen_mut();
            let offset = screen.scrollback();
            screen.set_scrollback(usize::MAX);
            let history = screen.scrollback();
            let (rows, cols) = screen.size();
            let rows = usize::from(rows);
            // Read the history a screen at a time, oldest first.
            let mut lines = Vec::with_capacity(history + rows);
            while lines.len() < history + rows {
                let back = history.saturating_sub(lines.len());
                screen.set_scrollback(back);
                let top = history - back;
                let skip = lines.len() - top;
                lines.extend(screen.rows(0, cols).skip(skip).map(|l| l.trim_end().to_string()));
            }
            screen.set_scrollback(offset);
            let cursor = if offset == 0 {
                history + usize::from(screen.cursor_position().0)
            } else {
                history - offset + rows - 1
            };
            CopyMode {
                lines,
                history,
                cursor,
                anchor: None,
                prompt: None,
                query: String::new(),
                pending_g: false,
                message: None,
            }
        };
        self.copy = Some(copy);
    }

    fn exit_copy_mode(&mut self) {
        self.copy = None;
        self.parser().screen_mut().set_scrollback(0);
    }

    /// Scrolls so that line `line` of the history mode snapshot is visible.
    fn show_line(&mut self, line: usize) {
        let Some(copy) = &self.copy else { return };
        let history = copy.history;
        let mut parser = self.parser();
        let screen = parser.screen_mut();
        let rows = usize::from(screen.size().0);
        let offset = screen.scrollback();
        let top = history.saturating_sub(offset);
        let offset = if line < top {
            history - line
        } else if line >= top + rows {
            history.saturating_sub(line + 1 - rows)
        } else {
            offset
        };
        screen.set_scrollback(offset.min(history));
    }

    /// Handles a key in history mode.
    pub fn copy_key(&mut self, key: KeyEvent) -> CopyAction {
        let Some(mut copy) = self.copy.take() else { return CopyAction::None };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let rows = usize::from(self.size.0);
        let last = copy.lines.len().saturating_sub(1);
        copy.message = None;
        let pending_g = std::mem::take(&mut copy.pending_g);

        if let Some(prompt) = &mut copy.prompt {
            match key.code {
                KeyCode::Enter => {
                    copy.query = std::mem::take(prompt);
                    copy.prompt = None;
                    match copy.find(true) {
                        Some(line) => copy.cursor = line,
                        None => copy.message = Some(format!("'{}' not found", copy.query)),
                    }
                }
                KeyCode::Esc => copy.prompt = None,
                KeyCode::Backspace => {
                    prompt.pop();
                }
                KeyCode::Char(c) if !ctrl => prompt.push(c),
                _ => {}
            }
        } else {
            let cursor = copy.cursor as isize;
            let (page, half) = (rows as isize, (rows / 2).max(1) as isize);
            let target = match key.code {
                KeyCode::Char('d') if ctrl => Some(cursor + half),
                KeyCode::Char('u') if ctrl => Some(cursor - half),
                KeyCode::Char('f') if ctrl => Some(cursor + page),
                KeyCode::Char('b') if ctrl => Some(cursor - page),
                KeyCode::Char('j') | KeyCode::Down => Some(cursor + 1),
                KeyCode::Char('k') | KeyCode::Up => Some(cursor - 1),
                KeyCode::PageDown => Some(cursor + page),
                KeyCode::PageUp => Some(cursor - page),
                KeyCode::Char('G') | KeyCode::End => Some(last as isize),
                KeyCode::Home => Some(0),
                KeyCode::Char('g') if pending_g => Some(0),
                _ => None,
            };
            if let Some(line) = target {
                copy.cursor = line.clamp(0, last as isize) as usize;
            } else {
                match key.code {
                    KeyCode::Char('q') => {
                        self.exit_copy_mode();
                        return CopyAction::Exit;
                    }
                    KeyCode::Esc if copy.anchor.is_some() => copy.anchor = None,
                    KeyCode::Esc => {
                        self.exit_copy_mode();
                        return CopyAction::Exit;
                    }
                    KeyCode::Char('y') | KeyCode::Enter => {
                        let text = copy.selected_text();
                        self.exit_copy_mode();
                        return CopyAction::Copy(text);
                    }
                    KeyCode::Char('g') => copy.pending_g = true,
                    KeyCode::Char('v' | 'V') => {
                        copy.anchor = if copy.anchor.is_some() { None } else { Some(copy.cursor) };
                    }
                    KeyCode::Char('/') => copy.prompt = Some(String::new()),
                    KeyCode::Char(c @ ('n' | 'N')) if !copy.query.is_empty() => match copy.find(c == 'n') {
                        Some(line) => copy.cursor = line,
                        None => copy.message = Some(format!("'{}' not found", copy.query)),
                    },
                    _ => {}
                }
            }
        }
        let cursor = copy.cursor;
        self.copy = Some(copy);
        self.show_line(cursor);
        CopyAction::None
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

/// Answers the queries programs send to their terminal (device attributes,
/// cursor position, modes…), which vt100 leaves to the embedder. Without the
/// answers programs like fish or neovim wait for a timeout before starting.
#[derive(Debug, Default)]
pub struct Responder {
    replies: Vec<u8>,
}

impl Responder {
    /// Replies generated since the last call.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }
}

impl vt100::Callbacks for Responder {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if let Some(reply) = csi_reply(screen, i1, i2, params, c) {
            self.replies.extend_from_slice(reply.as_bytes());
        }
    }
}

/// The answer an xterm-like terminal gives to a CSI query, if it is one.
fn csi_reply(
    screen: &vt100::Screen,
    i1: Option<u8>,
    i2: Option<u8>,
    params: &[&[u16]],
    c: char,
) -> Option<String> {
    let param = |i: usize| params.get(i).and_then(|p| p.first()).copied().unwrap_or(0);
    let (row, col) = screen.cursor_position();
    match (i1, i2, c) {
        // Primary device attributes: a VT220 with ANSI colors.
        (None, None, 'c') if param(0) == 0 => Some("\x1b[?62;22c".into()),
        // Secondary device attributes.
        (Some(b'>'), None, 'c') if param(0) == 0 => Some("\x1b[>1;10;0c".into()),
        // Device status report and cursor position report.
        (None, None, 'n') if param(0) == 5 => Some("\x1b[0n".into()),
        (None, None, 'n') if param(0) == 6 => Some(format!("\x1b[{};{}R", row + 1, col + 1)),
        (Some(b'?'), None, 'n') if param(0) == 6 => Some(format!("\x1b[?{};{}R", row + 1, col + 1)),
        // Size of the text area in characters.
        (None, None, 't') if param(0) == 18 => {
            let (rows, cols) = screen.size();
            Some(format!("\x1b[8;{rows};{cols}t"))
        }
        // Terminal name and version (XTVERSION).
        (Some(b'>'), None, 'q') if param(0) == 0 => {
            Some(format!("\x1bP>|sshh {}\x1b\\", env!("CARGO_PKG_VERSION")))
        }
        // Mode queries (DECRQM): 1 = set, 2 = reset, 0 = not recognized.
        (Some(b'?'), Some(b'$'), 'p') => {
            let mode = param(0);
            let state = |on: bool| if on { 1 } else { 2 };
            let status = match mode {
                1 => state(screen.application_cursor()),
                25 => state(!screen.hide_cursor()),
                47 | 1047 | 1049 => state(screen.alternate_screen()),
                2004 => state(screen.bracketed_paste()),
                _ => 0,
            };
            Some(format!("\x1b[?{mode};{status}$y"))
        }
        (Some(b'$'), None, 'p') => Some(format!("\x1b[{};0$y", param(0))),
        _ => None,
    }
}

/// Bytes an xterm-compatible terminal sends for a mouse event, according to
/// the mode and encoding the program enabled. `None` if the mode doesn't report
/// this event (or the position can't be encoded).
pub fn mouse_bytes(
    event: MouseEvent,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    use MouseEventKind::*;
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (code, release, motion) = match event.kind {
        Down(b) => (button(b), false, false),
        Up(b) => (button(b), true, false),
        Drag(b) => (button(b) + 32, false, true),
        Moved => (3 + 32, false, true),
        ScrollUp => (64, false, false),
        ScrollDown => (65, false, false),
        ScrollLeft => (66, false, false),
        ScrollRight => (67, false, false),
    };
    let reported = match mode {
        MouseProtocolMode::None => false,
        MouseProtocolMode::Press => !release && !motion,
        MouseProtocolMode::PressRelease => !motion,
        MouseProtocolMode::ButtonMotion => !matches!(event.kind, Moved),
        MouseProtocolMode::AnyMotion => true,
    };
    if !reported {
        return None;
    }
    let mut code = code;
    if mode != MouseProtocolMode::Press {
        let m = event.modifiers;
        code += 4 * u16::from(m.contains(KeyModifiers::SHIFT))
            + 8 * u16::from(m.contains(KeyModifiers::ALT))
            + 16 * u16::from(m.contains(KeyModifiers::CONTROL));
    }
    let (x, y) = (event.column + 1, event.row + 1);
    match encoding {
        MouseProtocolEncoding::Sgr => {
            let end = if release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes())
        }
        // Without SGR, a release doesn't say which button (3 = "released").
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let code = if release { 3 + (code & !3) } else { code };
            let mut bytes = b"\x1b[M".to_vec();
            for value in [code, x, y] {
                let value = u32::from(value) + 32;
                if encoding == MouseProtocolEncoding::Default {
                    bytes.push(u8::try_from(value).ok()?);
                } else {
                    let mut buf = [0; 4];
                    bytes.extend_from_slice(char::from_u32(value)?.encode_utf8(&mut buf).as_bytes());
                }
            }
            Some(bytes)
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

    fn answers(input: &[u8]) -> String {
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, Responder::default());
        parser.process(input);
        String::from_utf8(parser.callbacks_mut().take()).unwrap()
    }

    #[test]
    fn answers_terminal_queries() {
        assert_eq!(answers(b"\x1b[c"), "\x1b[?62;22c");
        assert_eq!(answers(b"\x1b[0c"), "\x1b[?62;22c");
        assert_eq!(answers(b"\x1b[>c"), "\x1b[>1;10;0c");
        assert_eq!(answers(b"\x1b[5n"), "\x1b[0n");
        assert_eq!(answers(b"\x1b[3;7H\x1b[6n"), "\x1b[3;7R");
        assert_eq!(answers(b"abc\x1b[?6n"), "\x1b[?1;4R");
        assert_eq!(answers(b"\x1b[18t"), "\x1b[8;24;80t");
        assert_eq!(answers(b"\x1b[>q"), format!("\x1bP>|sshh {}\x1b\\", env!("CARGO_PKG_VERSION")));
        assert_eq!(answers(b"\x1b[?2004h\x1b[?2004$p"), "\x1b[?2004;1$y");
        assert_eq!(answers(b"\x1b[?2026$p"), "\x1b[?2026;0$y");
        // Ordinary output and unknown sequences get no answer.
        assert_eq!(answers(b"hello\x1b[31mred\x1b[0m\x1b[?u"), "");
    }

    #[test]
    fn programs_get_their_answers() {
        // bash asks for the cursor position and prints the answer it reads.
        let dirty = Arc::new(AtomicBool::new(false));
        let session = Session::spawn(
            Path::new("bash"),
            &["-c".into(), r"printf '\033[6n'; IFS= read -rs -d R pos; echo got:${pos#*[}".into()],
            10,
            40,
            dirty,
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.parser().screen().contents().contains("got:1;1") {
            assert!(std::time::Instant::now() < deadline, "{}", session.parser().screen().contents());
            thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn mouse_encodings_and_modes() {
        use MouseProtocolEncoding as E;
        use MouseProtocolMode as M;
        let ev = |kind, column, row, modifiers| MouseEvent { kind, column, row, modifiers };
        let none = KeyModifiers::NONE;
        let down = ev(MouseEventKind::Down(MouseButton::Left), 4, 9, none);
        let up = ev(MouseEventKind::Up(MouseButton::Left), 4, 9, none);
        let wheel = ev(MouseEventKind::ScrollDown, 0, 0, KeyModifiers::CONTROL);
        let drag = ev(MouseEventKind::Drag(MouseButton::Right), 1, 1, none);
        let moved = ev(MouseEventKind::Moved, 1, 1, none);

        assert_eq!(mouse_bytes(down, M::PressRelease, E::Sgr).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(mouse_bytes(up, M::PressRelease, E::Sgr).unwrap(), b"\x1b[<0;5;10m");
        assert_eq!(mouse_bytes(wheel, M::PressRelease, E::Sgr).unwrap(), b"\x1b[<81;1;1M");
        assert_eq!(mouse_bytes(down, M::PressRelease, E::Default).unwrap(), b"\x1b[M %*");
        assert_eq!(mouse_bytes(up, M::PressRelease, E::Default).unwrap(), b"\x1b[M#%*");
        assert_eq!(mouse_bytes(drag, M::ButtonMotion, E::Sgr).unwrap(), b"\x1b[<34;2;2M");

        // Modes filter what is reported.
        assert_eq!(mouse_bytes(down, M::None, E::Sgr), None);
        assert_eq!(mouse_bytes(up, M::Press, E::Sgr), None);
        assert_eq!(mouse_bytes(drag, M::PressRelease, E::Sgr), None);
        assert_eq!(mouse_bytes(moved, M::ButtonMotion, E::Sgr), None);
        assert_eq!(mouse_bytes(moved, M::AnyMotion, E::Sgr).unwrap(), b"\x1b[<35;2;2M");

        // Far positions: not encodable in the default encoding, fine in UTF-8.
        let far = ev(MouseEventKind::Down(MouseButton::Left), 300, 0, none);
        assert_eq!(mouse_bytes(far, M::PressRelease, E::Default), None);
        assert_eq!(mouse_bytes(far, M::PressRelease, E::Utf8).unwrap(), "\x1b[M \u{14d}!".as_bytes());
    }

    /// A session that printed `count` numbered lines and is waiting.
    fn session_with_lines(count: usize) -> Session {
        let dirty = Arc::new(AtomicBool::new(false));
        let script = format!("for i in $(seq 1 {count}); do echo line$i; done; sleep 30");
        let session =
            Session::spawn(Path::new("/bin/sh"), &["-c".into(), script], 10, 40, dirty).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.parser().screen().contents().contains(&format!("line{count}")) {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(20));
        }
        session
    }

    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn keys(session: &mut Session, text: &str) -> CopyAction {
        let mut action = CopyAction::None;
        for c in text.chars() {
            action = session.copy_key(plain(KeyCode::Char(c)));
        }
        action
    }

    #[test]
    fn history_mode_moves_searches_and_copies() {
        let mut session = session_with_lines(50);
        session.start_copy_mode();
        let copy = session.copy_mode().unwrap();
        // 50 lines + the empty cursor line; the cursor starts on the last one.
        assert_eq!((copy.total(), copy.cursor), (51, 50));

        keys(&mut session, "k");
        let cursor = |s: &Session| s.copy_mode().unwrap().cursor;
        assert_eq!(cursor(&session), 49);
        // "line4" matches line4 and line40..line49: search goes backwards.
        keys(&mut session, "/LINE4");
        session.copy_key(plain(KeyCode::Enter));
        assert_eq!(cursor(&session), 48);
        keys(&mut session, "n");
        assert_eq!(cursor(&session), 47);
        keys(&mut session, "N");
        assert_eq!(cursor(&session), 48);
        // A match above the screen scrolls the view back to it.
        keys(&mut session, "/line7");
        session.copy_key(plain(KeyCode::Enter));
        assert_eq!(cursor(&session), 6);
        let offset = session.parser().screen().scrollback();
        assert_eq!(session.copy_mode().unwrap().top(offset), 6);

        // Select three lines and copy them.
        keys(&mut session, "ggjv");
        let action = keys(&mut session, "jjy");
        assert_eq!(action, CopyAction::Copy("line2\nline3\nline4".into()));
        assert!(session.copy_mode().is_none());
        assert_eq!(session.parser().screen().scrollback(), 0);

        // Unknown searches report it; q leaves.
        session.start_copy_mode();
        keys(&mut session, "/nope");
        session.copy_key(plain(KeyCode::Enter));
        assert_eq!(session.copy_mode().unwrap().message.as_deref(), Some("'nope' not found"));
        assert_eq!(keys(&mut session, "q"), CopyAction::Exit);
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
