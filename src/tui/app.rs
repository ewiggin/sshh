//! TUI state and event handling (no dependency on a real terminal).
//!
//! Side-effecting operations (database, clipboard, $EDITOR, ssh sessions)
//! don't happen here: they are left in `request` and run by the main loop.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::TableState;

use super::form::{self, Form, FormEvent, FormKind};
use crate::db::HistoryEntry;
use crate::model::{Host, HostData};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How the TUI ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Quit,
}

/// Program run in the full terminal, suspending the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum External {
    Ssh,
    Sftp,
    SshCopyId,
}

/// Operation the main loop must run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Save { id: Option<i64>, data: Box<HostData> },
    Delete { alias: String },
    Copy(Box<HostData>),
    /// Edit the focused form field with $EDITOR.
    Editor,
    Reload,
    /// Open (or reopen, if it ended) the embedded session of a connection.
    OpenSession(Box<Host>),
    CloseSession(i64),
    /// Input for the embedded session of the selected connection.
    Input(KeyEvent),
    Paste(String),
    /// Scroll the selected session's history (positive = back).
    Scroll(isize),
    External { program: External, host: Box<Host> },
}

/// State of the embedded session of a connection, kept in sync by the main loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Alive,
    Ended,
}

/// Footer message; cleared on the next key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

pub enum Mode {
    Normal,
    Search,
    Help,
    Form(Box<Form>),
    ConfirmDelete { alias: String },
    ConfirmClose { id: i64, alias: String },
    ConfirmQuit { sessions: usize },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    Recent,
    MostUsed,
    Alias,
}

impl SortOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::MostUsed => "most used",
            Self::Alias => "alphabetical",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Recent => Self::MostUsed,
            Self::MostUsed => Self::Alias,
            Self::Alias => Self::Recent,
        }
    }

    fn compare(self, a: &Host, b: &Host) -> std::cmp::Ordering {
        // `Reverse(Option)`: None (never used) goes last.
        let recent = |h: &Host| std::cmp::Reverse(h.last_used);
        let alias = |h: &Host| h.data.alias.to_lowercase();
        match self {
            Self::Recent => recent(a).cmp(&recent(b)).then_with(|| alias(a).cmp(&alias(b))),
            Self::MostUsed => b
                .use_count
                .cmp(&a.use_count)
                .then_with(|| recent(a).cmp(&recent(b)))
                .then_with(|| alias(a).cmp(&alias(b))),
            Self::Alias => alias(a).cmp(&alias(b)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    List,
    Detail,
    /// The embedded terminal: keys go to the ssh session.
    Terminal,
}

/// Visible row after filtering, with the positions (in chars) matching the
/// search so they can be highlighted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub index: usize,
    pub alias_hl: Vec<usize>,
    pub name_hl: Vec<usize>,
}

pub struct App {
    pub hosts: Vec<Host>,
    pub entries: Vec<Entry>,
    pub query: String,
    pub mode: Mode,
    pub focus: Focus,
    pub sort: SortOrder,
    pub table: TableState,
    pub detail_scroll: u16,
    /// First key of a two-key shortcut (`gg`, `dd`, `yy`).
    pub pending: Option<char>,
    pub status: Option<Status>,
    pub request: Option<Request>,
    pub outcome: Option<Outcome>,
    /// Latest connections of `history_for` (loaded by the main loop).
    pub history: Vec<HistoryEntry>,
    pub history_for: Option<i64>,
    /// Embedded sessions by connection id (kept in sync by the main loop).
    pub sessions: HashMap<i64, SessionState>,
    /// Areas from the last render (for the mouse and to size sessions).
    pub rows_area: Rect,
    pub search_area: Rect,
    pub detail_area: Rect,
    pub terminal_area: Rect,
    matcher: Matcher,
    last_click: Option<(usize, Instant)>,
}

impl App {
    pub fn new(hosts: Vec<Host>) -> Self {
        let mut app = Self {
            hosts,
            entries: Vec::new(),
            query: String::new(),
            mode: Mode::Normal,
            focus: Focus::List,
            sort: SortOrder::default(),
            table: TableState::default(),
            detail_scroll: 0,
            pending: None,
            status: None,
            request: None,
            outcome: None,
            history: Vec::new(),
            history_for: None,
            sessions: HashMap::new(),
            rows_area: Rect::default(),
            search_area: Rect::default(),
            detail_area: Rect::default(),
            terminal_area: Rect::default(),
            matcher: Matcher::new(Config::DEFAULT),
            last_click: None,
        };
        app.refilter();
        app.select(0);
        app
    }

    pub fn selected_host(&self) -> Option<&Host> {
        let entry = self.entries.get(self.table.selected()?)?;
        self.hosts.get(entry.index)
    }

    /// Session state of the selected connection, if it has one.
    pub fn selected_session(&self) -> Option<SessionState> {
        self.sessions.get(&self.selected_host()?.id).copied()
    }

    fn alive_sessions(&self) -> usize {
        self.sessions.values().filter(|s| **s == SessionState::Alive).count()
    }

    /// Replaces the list (after save/delete/reload), trying to keep connection
    /// `select_id` selected or, failing that, the same position.
    pub fn set_hosts(&mut self, hosts: Vec<Host>, select_id: Option<i64>) {
        let previous = self.table.selected().unwrap_or(0);
        self.hosts = hosts;
        self.refilter();
        let by_id = select_id.and_then(|id| {
            self.entries.iter().position(|e| self.hosts[e.index].id == id)
        });
        self.select(by_id.unwrap_or(previous));
    }

    /// Result of a `Save` request.
    pub fn on_saved(&mut self, result: Result<(i64, Vec<Host>), String>) {
        match result {
            Ok((id, hosts)) => {
                self.mode = Mode::Normal;
                self.set_hosts(hosts, Some(id));
                let alias = self.selected_host().map(|h| h.data.alias.clone()).unwrap_or_default();
                self.info(format!("Connection '{alias}' saved"));
            }
            Err(msg) => {
                if let Mode::Form(form) = &mut self.mode {
                    form.error = Some(msg);
                }
            }
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: false });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: true });
    }

    fn select(&mut self, index: usize) {
        let selected = self.entries.len().checked_sub(1).map(|last| index.min(last));
        if selected != self.table.selected() {
            self.detail_scroll = 0;
        }
        self.table.select(selected);
    }

    /// Recomputes the visible rows. `#tag` words filter by tag (prefix match)
    /// and the rest is fuzzy searched.
    fn refilter(&mut self) {
        let (tags, words): (Vec<&str>, Vec<&str>) =
            self.query.split_whitespace().partition(|w| w.starts_with('#'));
        let tags: Vec<String> = tags
            .iter()
            .map(|t| t[1..].to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        let words = words.join(" ");
        let pattern = Pattern::parse(&words, CaseMatching::Smart, Normalization::Smart);

        let mut order: Vec<usize> = (0..self.hosts.len()).collect();
        order.sort_by(|&a, &b| self.sort.compare(&self.hosts[a], &self.hosts[b]));

        let mut scored = Vec::new();
        let mut buf = Vec::new();
        let mut indices = Vec::new();
        for index in order {
            let host = &self.hosts[index];
            let has_tags = tags.iter().all(|t| {
                host.data.tags.iter().any(|ht| ht.to_lowercase().starts_with(t.as_str()))
            });
            if !has_tags {
                continue;
            }
            let d = &host.data;
            let alias_len = d.alias.chars().count();
            let name = d.name.as_deref().unwrap_or_default();
            let name_start = alias_len + 1;
            let name_end = name_start + name.chars().count();
            // The first two fields must be alias and name (see name_start).
            let haystack = [
                d.alias.as_str(),
                name,
                d.user.as_deref().unwrap_or_default(),
                &d.hostname,
                d.description.as_deref().unwrap_or_default(),
                &d.tags.join(" "),
            ]
            .join(" ");
            indices.clear();
            let Some(score) =
                pattern.indices(Utf32Str::new(&haystack, &mut buf), &mut self.matcher, &mut indices)
            else {
                continue;
            };
            indices.sort_unstable();
            indices.dedup();
            let pos = indices.iter().map(|&i| i as usize);
            scored.push((
                score,
                Entry {
                    index,
                    alias_hl: pos.clone().filter(|&i| i < alias_len).collect(),
                    name_hl: pos
                        .filter(|&i| (name_start..name_end).contains(&i))
                        .map(|i| i - name_start)
                        .collect(),
                },
            ));
        }
        if !words.trim().is_empty() {
            // Stable: equal scores keep the chosen order.
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        self.entries = scored.into_iter().map(|(_, e)| e).collect();
    }

    /// After the query changes, the best result is selected.
    fn query_changed(&mut self) {
        self.refilter();
        self.select(0);
        *self.table.offset_mut() = 0;
    }

    fn move_by(&mut self, delta: isize) {
        let current = self.table.selected().unwrap_or(0) as isize;
        self.select(current.saturating_add(delta).max(0) as usize);
    }

    fn scroll_detail(&mut self, delta: isize) {
        self.detail_scroll = (self.detail_scroll as isize + delta).clamp(0, u16::MAX as isize) as u16;
    }

    fn page(&self) -> isize {
        self.rows_area.height.max(1) as isize
    }

    /// Opens (or focuses) the embedded session of the selected connection.
    fn connect_selected(&mut self) {
        let Some(host) = self.selected_host() else { return };
        if self.selected_session() != Some(SessionState::Alive) {
            self.request = Some(Request::OpenSession(Box::new(host.clone())));
        }
        self.mode = Mode::Normal;
        self.focus = Focus::Terminal;
    }

    fn quit(&mut self) {
        match self.alive_sessions() {
            0 => self.outcome = Some(Outcome::Quit),
            sessions => self.mode = Mode::ConfirmQuit { sessions },
        }
    }

    /// Moves the selection to the next (or previous) connection with a session.
    fn switch_session(&mut self, forward: bool) {
        let with_session: Vec<usize> = (0..self.entries.len())
            .filter(|&i| self.sessions.contains_key(&self.hosts[self.entries[i].index].id))
            .collect();
        let current = self.table.selected().unwrap_or(0);
        let next = if forward {
            with_session.iter().find(|&&i| i > current).or(with_session.first())
        } else {
            with_session.iter().rev().find(|&&i| i < current).or(with_session.last())
        };
        if let Some(&next) = next {
            self.select(next);
        }
    }

    /// Alt-h/j/k/l: move between panes and sessions from anywhere.
    fn on_alt_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        match key.code {
            KeyCode::Char('h') => self.focus = Focus::List,
            KeyCode::Char('l') if self.selected_session().is_some() => self.focus = Focus::Terminal,
            KeyCode::Char('l') => {}
            KeyCode::Char('j') => self.switch_session(true),
            KeyCode::Char('k') => self.switch_session(false),
            _ => return false,
        }
        if self.focus == Focus::Terminal && self.selected_session().is_none() {
            self.focus = Focus::List;
        }
        true
    }

    /// Keys while the embedded terminal has focus: everything goes to ssh.
    fn on_terminal_key(&mut self, key: KeyEvent) {
        match self.selected_session() {
            Some(SessionState::Alive) => self.request = Some(Request::Input(key)),
            Some(SessionState::Ended) if key.code == KeyCode::Enter => self.connect_selected(),
            Some(SessionState::Ended) if key.code == KeyCode::Esc => self.focus = Focus::List,
            Some(SessionState::Ended) => {}
            None => self.focus = Focus::List,
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::Normal if self.focus == Focus::Terminal => {
                if self.selected_session() == Some(SessionState::Alive) {
                    self.request = Some(Request::Paste(text.to_string()));
                }
            }
            Mode::Search => {
                self.query.extend(text.chars().filter(|c| !c.is_control()));
                self.query_changed();
            }
            Mode::Form(form) => form.paste(text),
            _ => {}
        }
    }

    fn open_form(&mut self, kind: FormKind, focus: usize) {
        let data = match kind {
            FormKind::Edit(_) => match self.selected_host() {
                Some(host) => host.data.clone(),
                None => return,
            },
            _ => HostData::default(),
        };
        self.mode = Mode::Form(Box::new(Form::new(kind, &data).focus(focus)));
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.status = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(self.mode, Mode::Normal) {
            if self.on_alt_key(key) {
                return;
            }
            if self.focus == Focus::Terminal {
                self.on_terminal_key(key);
                return;
            }
        }
        match &mut self.mode {
            Mode::Form(form) => match form.on_key(key) {
                FormEvent::None => {}
                FormEvent::Cancel => self.mode = Mode::Normal,
                FormEvent::Editor => self.request = Some(Request::Editor),
                FormEvent::Submit => {
                    let id = match form.kind {
                        FormKind::Edit(id) => Some(id),
                        _ => None,
                    };
                    if let Some(data) = form.submit() {
                        self.request = Some(Request::Save { id, data: Box::new(data) });
                    }
                }
            },
            Mode::ConfirmDelete { alias } => {
                if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                    self.request = Some(Request::Delete { alias: alias.clone() });
                }
                self.mode = Mode::Normal;
            }
            Mode::ConfirmClose { id, .. } => {
                if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                    self.request = Some(Request::CloseSession(*id));
                    self.focus = Focus::List;
                }
                self.mode = Mode::Normal;
            }
            Mode::ConfirmQuit { .. } => {
                if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                    self.outcome = Some(Outcome::Quit);
                }
                self.mode = Mode::Normal;
            }
            _ if ctrl && key.code == KeyCode::Char('c') => self.quit(),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Search => self.on_search_key(key, ctrl),
            Mode::Normal => self.on_normal_key(key, ctrl),
        }
    }

    fn on_normal_key(&mut self, key: KeyEvent, ctrl: bool) {
        let pending = self.pending.take();
        if self.focus == Focus::Detail && self.on_detail_key(key, ctrl) {
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit(),
            // Esc goes back one level; it never quits the app.
            KeyCode::Esc if pending.is_none() && !self.query.is_empty() => {
                self.query.clear();
                self.query_changed();
            }
            KeyCode::Enter => self.connect_selected(),
            KeyCode::Char('/') => self.mode = Mode::Search,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Tab => self.cycle_focus(true),
            KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::Char('d') if ctrl => self.move_by(self.page() / 2),
            KeyCode::Char('u') if ctrl => self.move_by(-self.page() / 2),
            KeyCode::Char('f') if ctrl => self.move_by(self.page()),
            KeyCode::Char('b') if ctrl => self.move_by(-self.page()),
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX),
            KeyCode::Home => self.select(0),
            KeyCode::PageDown => self.move_by(self.page()),
            KeyCode::PageUp => self.move_by(-self.page()),
            KeyCode::Char('a') => self.open_form(FormKind::Add, form::ALIAS),
            KeyCode::Char('e') => self.edit_selected(form::ALIAS),
            KeyCode::Char('t') => self.edit_selected(form::TAGS),
            KeyCode::Char('R') => self.request = Some(Request::Reload),
            KeyCode::Char('f') if !ctrl => self.run_selected(External::Ssh),
            KeyCode::Char('s') => self.run_selected(External::Sftp),
            KeyCode::Char('c') => self.run_selected(External::SshCopyId),
            KeyCode::Char('x') => self.close_selected(),
            KeyCode::Char('o') => {
                self.sort = self.sort.next();
                let selected = self.selected_host().map(|h| h.id);
                let hosts = std::mem::take(&mut self.hosts);
                self.set_hosts(hosts, selected);
                self.info(format!("Sort: {}", self.sort.label()));
            }
            KeyCode::Char(c @ ('g' | 'd' | 'y')) if !ctrl => {
                if pending == Some(c) {
                    self.double_key(c);
                } else {
                    self.pending = Some(c);
                }
            }
            _ => {}
        }
    }

    /// Tab / Shift-Tab: list → details → terminal (only if the selected
    /// connection has a session) and back. Inside the terminal Tab goes to ssh.
    fn cycle_focus(&mut self, forward: bool) {
        let mut panes = vec![Focus::List, Focus::Detail];
        if self.selected_session().is_some() {
            panes.push(Focus::Terminal);
        }
        let current = panes.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward { current + 1 } else { current + panes.len() - 1 };
        self.focus = panes[next % panes.len()];
    }

    /// Keys while the detail pane has focus; returns false if not consumed.
    fn on_detail_key(&mut self, key: KeyEvent, ctrl: bool) -> bool {
        let half = (self.detail_area.height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_detail(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_detail(-1),
            KeyCode::Char('d') if ctrl => self.scroll_detail(half),
            KeyCode::Char('u') if ctrl => self.scroll_detail(-half),
            KeyCode::Esc => self.focus = Focus::List,
            _ => return false,
        }
        true
    }

    fn run_selected(&mut self, program: External) {
        if let Some(host) = self.selected_host() {
            self.request = Some(Request::External { program, host: Box::new(host.clone()) });
        }
    }

    /// Closes the selected connection's session (asking first if it's alive).
    fn close_selected(&mut self) {
        let Some(host) = self.selected_host() else { return };
        let (id, alias) = (host.id, host.data.alias.clone());
        match self.selected_session() {
            Some(SessionState::Alive) => self.mode = Mode::ConfirmClose { id, alias },
            Some(SessionState::Ended) => self.request = Some(Request::CloseSession(id)),
            None => {}
        }
    }

    fn edit_selected(&mut self, focus: usize) {
        if let Some(id) = self.selected_host().map(|h| h.id) {
            self.open_form(FormKind::Edit(id), focus);
        }
    }

    fn double_key(&mut self, c: char) {
        match c {
            'g' => self.select(0),
            'd' => {
                if let Some(host) = self.selected_host() {
                    self.mode = Mode::ConfirmDelete { alias: host.data.alias.clone() };
                }
            }
            'y' => {
                if let Some(host) = self.selected_host() {
                    self.request = Some(Request::Copy(Box::new(host.data.clone())));
                }
            }
            _ => {}
        }
    }

    fn on_search_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => self.connect_selected(),
            KeyCode::Down => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Char('n' | 'j') if ctrl => self.move_by(1),
            KeyCode::Char('p' | 'k') if ctrl => self.move_by(-1),
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.query_changed();
            }
            KeyCode::Char('w') if ctrl => {
                let trimmed = self.query.trim_end();
                let cut = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
                self.query.truncate(cut);
                self.query_changed();
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.query_changed();
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.query.push(c);
                self.query_changed();
            }
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        let pos = Position::new(mouse.column, mouse.row);
        let in_detail = self.detail_area.contains(pos);
        let in_terminal = self.terminal_area.contains(pos) && self.selected_session().is_some();
        match (&mut self.mode, mouse.kind) {
            (Mode::Form(form), MouseEventKind::Down(MouseButton::Left)) => form.on_click(pos),
            (Mode::Form(_) | Mode::ConfirmDelete { .. } | Mode::ConfirmClose { .. }, _) => {}
            (Mode::ConfirmQuit { .. }, _) => {}
            (Mode::Help, MouseEventKind::Down(_)) => self.mode = Mode::Normal,
            (Mode::Help, _) => {}
            (_, MouseEventKind::ScrollDown) if in_terminal => self.request = Some(Request::Scroll(-3)),
            (_, MouseEventKind::ScrollUp) if in_terminal => self.request = Some(Request::Scroll(3)),
            (_, MouseEventKind::ScrollDown) if in_detail => self.scroll_detail(1),
            (_, MouseEventKind::ScrollUp) if in_detail => self.scroll_detail(-1),
            (_, MouseEventKind::ScrollDown) => self.move_by(1),
            (_, MouseEventKind::ScrollUp) => self.move_by(-1),
            (_, MouseEventKind::Down(MouseButton::Left)) => {
                if self.search_area.contains(pos) {
                    self.mode = Mode::Search;
                } else if in_terminal {
                    self.mode = Mode::Normal;
                    self.focus = Focus::Terminal;
                } else if in_detail {
                    self.mode = Mode::Normal;
                    self.focus = Focus::Detail;
                } else if self.rows_area.contains(pos) {
                    self.click_row(self.table.offset() + usize::from(pos.y - self.rows_area.y));
                }
            }
            _ => {}
        }
    }

    /// A click selects the row; a double click on the same row connects.
    fn click_row(&mut self, row: usize) {
        if row >= self.entries.len() {
            return;
        }
        self.mode = Mode::Normal;
        self.focus = Focus::List;
        self.select(row);
        let double = self
            .last_click
            .is_some_and(|(r, at)| r == row && at.elapsed() < DOUBLE_CLICK);
        if double {
            self.last_click = None;
            self.connect_selected();
        } else {
            self.last_click = Some((row, Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: i64, alias: &str, name: &str, tags: &[&str]) -> Host {
        Host {
            id,
            data: HostData {
                alias: alias.into(),
                hostname: format!("{alias}.example.com"),
                name: (!name.is_empty()).then(|| name.into()),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                ..Default::default()
            },
            created_at: 0,
            updated_at: 0,
            last_used: None,
            use_count: 0,
        }
    }

    /// Sorted by last use: web1, db1, dev.
    fn app() -> App {
        let mut hosts = vec![
            host(1, "web1", "Web production", &["prod", "web"]),
            host(2, "db1", "Database", &["prod", "db"]),
            host(3, "dev", "", &["dev"]),
        ];
        for (host, t) in hosts.iter_mut().zip([30, 20, 10]) {
            host.last_used = Some(t);
        }
        App::new(hosts)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_query(app: &mut App, q: &str) {
        if !matches!(app.mode, Mode::Search) {
            app.on_key(ch('/'));
        }
        for c in q.chars() {
            app.on_key(ch(c));
        }
    }

    fn aliases(app: &App) -> Vec<&str> {
        app.entries.iter().map(|e| app.hosts[e.index].data.alias.as_str()).collect()
    }

    #[test]
    fn empty_query_keeps_order() {
        assert_eq!(aliases(&app()), ["web1", "db1", "dev"]);
    }

    #[test]
    fn fuzzy_search_and_highlight() {
        let mut app = app();
        type_query(&mut app, "dat");
        assert_eq!(aliases(&app)[0], "db1");
        assert_eq!(app.entries[0].name_hl, [0, 1, 2]);
        assert!(app.entries[0].alias_hl.is_empty());
    }

    #[test]
    fn tag_filter() {
        let mut app = app();
        type_query(&mut app, "#pro");
        assert_eq!(aliases(&app), ["web1", "db1"]);
        type_query(&mut app, " db");
        // Fuzzy: web1 matches too ("proDuction … weB1"), but scores lower.
        assert_eq!(aliases(&app)[0], "db1");
        type_query(&mut app, " #db");
        assert_eq!(aliases(&app), ["db1"]);
    }

    #[test]
    fn vim_navigation() {
        let mut app = app();
        app.rows_area = Rect::new(0, 0, 40, 10);
        app.on_key(ch('k'));
        assert_eq!(app.table.selected(), Some(0));
        app.on_key(ch('G'));
        assert_eq!(app.table.selected(), Some(2));
        app.on_key(ch('j'));
        assert_eq!(app.table.selected(), Some(2));
        // A single `g` doesn't move; `gg` goes to the top.
        app.on_key(ch('g'));
        assert_eq!(app.table.selected(), Some(2));
        app.on_key(ch('g'));
        assert_eq!(app.table.selected(), Some(0));
        // `g` followed by another key cancels the prefix.
        app.on_key(ch('g'));
        app.on_key(ch('j'));
        app.on_key(ch('g'));
        assert_eq!(app.table.selected(), Some(1));
        app.on_key(ctrl('f'));
        assert_eq!(app.table.selected(), Some(2));
        app.on_key(ctrl('b'));
        assert_eq!(app.table.selected(), Some(0));
    }

    #[test]
    fn esc_never_quits() {
        let mut app = app();
        type_query(&mut app, "dev");
        app.on_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.query, "");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.outcome, None);
        app.on_key(ch('q'));
        assert_eq!(app.outcome, Some(Outcome::Quit));
    }

    #[test]
    fn search_ctrl_jk_and_enter_connects() {
        let mut app = app();
        type_query(&mut app, "#prod");
        app.on_key(ctrl('j'));
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(&app.request, Some(Request::OpenSession(h)) if h.data.alias == "db1"));
        assert_eq!(app.focus, Focus::Terminal);
    }

    #[test]
    fn dd_asks_before_deleting() {
        let mut app = app();
        app.on_key(ch('j'));
        app.on_key(ch('d'));
        assert!(matches!(app.mode, Mode::Normal));
        app.on_key(ch('d'));
        assert!(matches!(&app.mode, Mode::ConfirmDelete { alias } if alias == "db1"));
        app.on_key(ch('n'));
        assert_eq!(app.request, None);

        app.on_key(ch('d'));
        app.on_key(ch('d'));
        app.on_key(ch('y'));
        assert_eq!(app.request, Some(Request::Delete { alias: "db1".into() }));
    }

    #[test]
    fn quick_actions() {
        for (c, program) in [('s', External::Sftp), ('c', External::SshCopyId), ('f', External::Ssh)] {
            let mut app = app();
            app.on_key(ch(c));
            assert!(
                matches!(&app.request, Some(Request::External { program: p, host }) if *p == program && host.id == 1),
                "{c}"
            );
        }
    }

    /// App with a live session on db1 (selected) and an ended one on dev.
    fn app_with_sessions() -> App {
        let mut app = app();
        app.sessions.insert(2, SessionState::Alive);
        app.sessions.insert(3, SessionState::Ended);
        app.on_key(ch('j'));
        app
    }

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    #[test]
    fn enter_focuses_an_alive_session_without_reopening() {
        let mut app = app_with_sessions();
        app.on_key(key(KeyCode::Enter));
        assert_eq!((app.focus, &app.request), (Focus::Terminal, &None));
    }

    #[test]
    fn terminal_focus_forwards_every_key() {
        let mut app = app_with_sessions();
        app.on_key(alt('l'));
        assert_eq!(app.focus, Focus::Terminal);
        for k in [ch('q'), ctrl('c'), key(KeyCode::Esc), ch('/')] {
            app.on_key(k);
            assert_eq!(app.request.take(), Some(Request::Input(k)));
        }
        assert_eq!(app.outcome, None);
        app.on_paste("ls\n");
        assert_eq!(app.request.take(), Some(Request::Paste("ls\n".into())));
        app.on_key(alt('h'));
        assert_eq!((app.focus, &app.request), (Focus::List, &None));
    }

    #[test]
    fn alt_l_needs_a_session() {
        let mut app = app();
        app.on_key(alt('l'));
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn alt_j_k_cycle_through_sessions() {
        let mut app = app_with_sessions();
        app.on_key(alt('j'));
        assert_eq!(app.selected_host().unwrap().id, 3);
        app.on_key(alt('j'));
        assert_eq!(app.selected_host().unwrap().id, 2);
        app.on_key(alt('k'));
        assert_eq!(app.selected_host().unwrap().id, 3);
    }

    #[test]
    fn ended_session_reconnects_on_enter() {
        let mut app = app_with_sessions();
        app.on_key(ch('j'));
        app.on_key(alt('l'));
        app.on_key(ch('a'));
        assert_eq!(app.request, None);
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(&app.request, Some(Request::OpenSession(h)) if h.id == 3));
    }

    #[test]
    fn closing_and_quitting_ask_when_sessions_are_alive() {
        let mut app = app_with_sessions();
        app.on_key(ch('x'));
        assert!(matches!(&app.mode, Mode::ConfirmClose { id: 2, .. }));
        app.on_key(ch('y'));
        assert_eq!(app.request.take(), Some(Request::CloseSession(2)));

        app.on_key(ch('q'));
        assert!(matches!(app.mode, Mode::ConfirmQuit { sessions: 1 }));
        app.on_key(ch('n'));
        assert_eq!(app.outcome, None);
        app.on_key(ctrl('c'));
        app.on_key(ch('y'));
        assert_eq!(app.outcome, Some(Outcome::Quit));
    }

    #[test]
    fn wheel_over_the_session_scrolls_its_history() {
        let mut app = app_with_sessions();
        app.terminal_area = Rect::new(40, 0, 80, 20);
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 50,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(wheel);
        assert_eq!(app.request, Some(Request::Scroll(3)));
    }

    #[test]
    fn yy_copies() {
        let mut app = app();
        app.on_key(ch('y'));
        app.on_key(ch('y'));
        assert!(matches!(app.request, Some(Request::Copy(ref d)) if d.alias == "web1"));
    }

    #[test]
    fn edit_form_submits_save_request() {
        let mut app = app();
        app.on_key(ch('t'));
        let Mode::Form(form) = &app.mode else { panic!("no form") };
        assert_eq!((form.kind, form.focus), (FormKind::Edit(1), form::TAGS));
        for c in ", nuevo".chars() {
            app.on_key(ch(c));
        }
        app.on_key(key(KeyCode::Enter));
        let Some(Request::Save { id: Some(1), data }) = &app.request else {
            panic!("expected Save: {:?}", app.request)
        };
        assert_eq!(data.tags, ["prod", "web", "nuevo"]);

        let mut hosts = app.hosts.clone();
        hosts[0].data = (**data).clone();
        app.on_saved(Ok((1, hosts)));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.selected_host().unwrap().data.tags.len(), 3);
    }

    #[test]
    fn save_error_stays_in_form() {
        let mut app = app();
        app.on_key(ch('a'));
        app.on_saved(Err("already exists".into()));
        let Mode::Form(form) = &app.mode else { panic!("no form") };
        assert_eq!(form.error.as_deref(), Some("already exists"));
    }

    #[test]
    fn sort_orders() {
        let mut hosts = vec![
            host(1, "b", "", &[]),
            host(2, "A", "", &[]),
            host(3, "c", "", &[]),
        ];
        hosts[0].last_used = Some(10);
        hosts[0].use_count = 1;
        hosts[2].last_used = Some(20);
        hosts[2].use_count = 1;
        hosts[1].last_used = Some(5);
        hosts[1].use_count = 7;
        let mut app = App::new(hosts);
        assert_eq!(aliases(&app), ["c", "b", "A"]);
        app.on_key(ch('o'));
        assert_eq!(app.sort, SortOrder::MostUsed);
        assert_eq!(aliases(&app), ["A", "c", "b"]);
        app.on_key(ch('o'));
        assert_eq!(aliases(&app), ["A", "b", "c"]);
        // The selection stays on the same connection.
        assert_eq!(app.selected_host().unwrap().data.alias, "c");
    }

    #[test]
    fn set_hosts_keeps_selection_by_id() {
        let mut app = app();
        app.on_key(ch('G'));
        let mut hosts = app.hosts.clone();
        hosts.rotate_left(1);
        app.set_hosts(hosts, Some(3));
        assert_eq!(app.selected_host().unwrap().id, 3);
    }

    #[test]
    fn tab_cycles_panes() {
        let mut app = app();
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Detail);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::List);
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.focus, Focus::Detail);

        // With a session the terminal joins the cycle; inside it Tab goes to ssh.
        let mut app = app_with_sessions();
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.focus, Focus::Terminal);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.request.take(), Some(Request::Input(key(KeyCode::Tab))));
        app.on_key(alt('h'));
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Terminal);
    }

    #[test]
    fn detail_focus_scrolls() {
        let mut app = app();
        app.on_key(key(KeyCode::Tab));
        app.on_key(ch('j'));
        app.on_key(ch('j'));
        assert_eq!((app.detail_scroll, app.table.selected()), (2, Some(0)));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.focus, Focus::List);
        app.on_key(ch('j'));
        assert_eq!((app.detail_scroll, app.table.selected()), (0, Some(1)));
    }

    #[test]
    fn double_click_connects() {
        let mut app = app();
        app.rows_area = Rect::new(0, 5, 40, 10);
        let click = |row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(click(6));
        assert_eq!(app.table.selected(), Some(1));
        assert_eq!(app.outcome, None);
        app.on_mouse(click(6));
        assert!(matches!(&app.request, Some(Request::OpenSession(h)) if h.id == 2));
        // Clicking outside the rows does nothing.
        app.on_mouse(click(14));
        assert_eq!(app.table.selected(), Some(1));
    }
}
