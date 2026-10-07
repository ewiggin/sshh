//! TUI state and event handling (no dependency on a real terminal).
//!
//! Side-effecting operations (database, clipboard, $EDITOR, ssh sessions)
//! don't happen here: they are left in `request` and run by the main loop.

use std::collections::{HashMap, HashSet};
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
/// Narrowest terminal column allowed when splitting.
const MIN_COLUMN_WIDTH: u16 = 40;
/// Columns reachable with Alt-1..Alt-9.
const MAX_COLUMNS: usize = 9;

/// How the TUI ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Quit,
}

/// Program run in the full terminal, suspending the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum External {
    Ssh,
    SshCopyId,
}

/// What runs in an embedded session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionKind {
    Ssh,
    Sftp,
}

impl SessionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Sftp => "sftp",
        }
    }
}

/// An embedded session: a connection and what runs in it. A connection can
/// have an ssh and an sftp session at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub host: i64,
    pub kind: SessionKind,
}

impl SessionKey {
    pub fn ssh(host: i64) -> Self {
        Self { host, kind: SessionKind::Ssh }
    }

    pub fn sftp(host: i64) -> Self {
        Self { host, kind: SessionKind::Sftp }
    }
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
    /// Open (or reopen, if it ended) an embedded session of a connection.
    OpenSession(Box<Host>, SessionKind),
    /// Close every session of a connection.
    CloseSessions(i64),
    /// Input for an embedded session.
    Input(SessionKey, KeyEvent),
    Paste(SessionKey, String),
    /// Scroll a session's history (positive = back).
    Scroll(SessionKey, isize),
    /// Mouse event for a session's program, relative to its screen.
    Mouse(SessionKey, MouseEvent),
    /// Enter history mode (move, search and copy lines) in a session.
    StartHistory(SessionKey),
    /// Key for a session in history mode.
    HistoryKey(SessionKey, KeyEvent),
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

/// Tabs of the list pane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ListTab {
    #[default]
    Connections,
    Tags,
}

/// A tag and how many connections have it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    List,
    Detail,
    /// The active terminal column: keys go to its ssh session.
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
    pub tab: ListTab,
    /// Every tag in use, alphabetically, for the Tags tab.
    pub tags: Vec<TagEntry>,
    pub tag_table: TableState,
    /// Where each tab title was drawn (for the mouse).
    pub tab_areas: Vec<(ListTab, Rect)>,
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
    pub sessions: HashMap<SessionKey, SessionState>,
    /// Sessions whose program asked for mouse events (kept in sync by the
    /// main loop).
    pub mouse_sessions: HashSet<SessionKey>,
    /// Sessions in history mode (kept in sync by the main loop).
    pub history_sessions: HashSet<SessionKey>,
    /// Sessions shown as terminal columns, left to right.
    pub columns: Vec<SessionKey>,
    pub active_column: usize,
    /// The active column takes the whole screen.
    pub zoomed: bool,
    /// Areas from the last render (for the mouse and to size sessions).
    pub rows_area: Rect,
    pub search_area: Rect,
    pub detail_area: Rect,
    /// Whole area for the terminal columns.
    pub columns_area: Rect,
    /// Inner area of every visible column, by column index.
    pub column_areas: Vec<(usize, Rect)>,
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
            tab: ListTab::default(),
            tags: Vec::new(),
            tag_table: TableState::default(),
            tab_areas: Vec::new(),
            table: TableState::default(),
            detail_scroll: 0,
            pending: None,
            status: None,
            request: None,
            outcome: None,
            history: Vec::new(),
            history_for: None,
            sessions: HashMap::new(),
            mouse_sessions: HashSet::new(),
            history_sessions: HashSet::new(),
            columns: Vec::new(),
            active_column: 0,
            zoomed: false,
            rows_area: Rect::default(),
            search_area: Rect::default(),
            detail_area: Rect::default(),
            columns_area: Rect::default(),
            column_areas: Vec::new(),
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
        self.host_session(self.selected_host()?.id)
    }

    /// Session state of a connection: alive if any of its sessions is.
    pub fn host_session(&self, host: i64) -> Option<SessionState> {
        let mut states = self.sessions.iter().filter(|(k, _)| k.host == host).map(|(_, s)| *s);
        let first = states.next()?;
        Some(if first == SessionState::Alive || states.any(|s| s == SessionState::Alive) {
            SessionState::Alive
        } else {
            SessionState::Ended
        })
    }

    pub fn host(&self, id: i64) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// Session shown in the active column.
    pub fn active_key(&self) -> Option<SessionKey> {
        self.columns.get(self.active_column).copied()
    }

    fn column_of(&self, key: SessionKey) -> Option<usize> {
        self.columns.iter().position(|c| *c == key)
    }

    /// Focuses terminal column `index` and selects its connection in the list.
    fn focus_column(&mut self, index: usize) {
        let Some(&key) = self.columns.get(index) else { return };
        self.active_column = index;
        self.mode = Mode::Normal;
        self.focus = Focus::Terminal;
        if let Some(row) = self.entries.iter().position(|e| self.hosts[e.index].id == key.host) {
            self.select(row);
        }
    }

    /// Removes the column showing session `key`, if any.
    pub fn remove_column_of(&mut self, key: SessionKey) {
        if let Some(index) = self.column_of(key) {
            self.remove_column(index);
        }
    }

    /// Removes every column of connection `host`.
    pub fn remove_columns_of_host(&mut self, host: i64) {
        while let Some(index) = self.columns.iter().position(|k| k.host == host) {
            self.remove_column(index);
        }
    }

    fn remove_column(&mut self, index: usize) {
        if index >= self.columns.len() {
            return;
        }
        self.columns.remove(index);
        if self.columns.is_empty() {
            self.active_column = 0;
            self.zoomed = false;
            if self.focus == Focus::Terminal {
                self.focus = Focus::List;
            }
        } else {
            self.active_column = self.active_column.min(self.columns.len() - 1);
            if index < self.active_column {
                self.active_column -= 1;
            }
        }
    }

    fn max_columns(&self) -> usize {
        match self.columns_area.width {
            0 => MAX_COLUMNS,
            width => usize::from(width / MIN_COLUMN_WIDTH).clamp(1, MAX_COLUMNS),
        }
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
        // Drop columns of connections that no longer exist.
        let gone: Vec<i64> =
            self.columns.iter().map(|k| k.host).filter(|id| self.host(*id).is_none()).collect();
        for id in gone {
            self.remove_columns_of_host(id);
        }
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
        self.refresh_tags();
    }

    /// Recomputes the Tags tab, keeping the same tag selected if it still exists.
    fn refresh_tags(&mut self) {
        let selected = self.selected_tag().map(|t| t.name.clone());
        let mut counts: Vec<TagEntry> = Vec::new();
        for tag in self.hosts.iter().flat_map(|h| &h.data.tags) {
            match counts.iter_mut().find(|t| t.name == *tag) {
                Some(entry) => entry.count += 1,
                None => counts.push(TagEntry { name: tag.clone(), count: 1 }),
            }
        }
        counts.sort_by_key(|t| t.name.to_lowercase());
        self.tags = counts;
        let index = selected
            .and_then(|name| self.tags.iter().position(|t| t.name == name))
            .unwrap_or_else(|| self.tag_table.selected().unwrap_or(0));
        self.select_tag(index);
    }

    pub fn selected_tag(&self) -> Option<&TagEntry> {
        self.tags.get(self.tag_table.selected()?)
    }

    fn select_tag(&mut self, index: usize) {
        let selected = self.tags.len().checked_sub(1).map(|last| index.min(last));
        if selected != self.tag_table.selected() {
            self.detail_scroll = 0;
        }
        self.tag_table.select(selected);
    }

    fn move_tag_by(&mut self, delta: isize) {
        let current = self.tag_table.selected().unwrap_or(0) as isize;
        self.select_tag(current.saturating_add(delta).max(0) as usize);
    }

    fn switch_tab(&mut self) {
        self.tab = match self.tab {
            ListTab::Connections => ListTab::Tags,
            ListTab::Tags => ListTab::Connections,
        };
        self.mode = Mode::Normal;
        self.focus = Focus::List;
        self.detail_scroll = 0;
    }

    /// Space on a tag: back to the connections, filtered by that tag.
    fn apply_tag(&mut self) {
        let Some(tag) = self.selected_tag() else { return };
        self.query = format!("#{}", tag.name);
        self.tab = ListTab::Connections;
        self.mode = Mode::Normal;
        self.focus = Focus::List;
        self.query_changed();
    }

    /// Keys of the Tags tab; returns false for the general keys it lets through.
    fn on_tags_key(&mut self, key: KeyEvent, ctrl: bool, pending: Option<char>) -> bool {
        let page = self.page();
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter => self.apply_tag(),
            KeyCode::Esc => self.tab = ListTab::Connections,
            KeyCode::Char('d') if ctrl => self.move_tag_by(page / 2),
            KeyCode::Char('u') if ctrl => self.move_tag_by(-page / 2),
            KeyCode::Char('f') if ctrl => self.move_tag_by(page),
            KeyCode::Char('b') if ctrl => self.move_tag_by(-page),
            KeyCode::Char('j') | KeyCode::Down => self.move_tag_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_tag_by(-1),
            KeyCode::Char('G') | KeyCode::End => self.move_tag_by(isize::MAX),
            KeyCode::Home => self.select_tag(0),
            KeyCode::PageDown => self.move_tag_by(page),
            KeyCode::PageUp => self.move_tag_by(-page),
            KeyCode::Char('g') if pending == Some('g') => self.select_tag(0),
            KeyCode::Char('g') => self.pending = Some('g'),
            KeyCode::Char('q' | '?' | '/' | '[' | ']' | 'a' | 'R') | KeyCode::Tab | KeyCode::BackTab => {
                return false;
            }
            // Connection actions don't apply here.
            _ => {}
        }
        true
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

    /// Space: shows the selected connection in the active column.
    fn connect_selected(&mut self) {
        self.open_in_column(SessionKind::Ssh, false);
    }

    /// Shows a session of the selected connection in the active column or,
    /// with `new_column`, in a new column right of it. Opens the session if
    /// needed. A session already shown in a column just gets focused.
    fn open_in_column(&mut self, kind: SessionKind, new_column: bool) {
        let Some(host) = self.selected_host().cloned() else { return };
        let key = SessionKey { host: host.id, kind };
        let index = match self.column_of(key) {
            Some(index) => {
                if new_column {
                    let (alias, kind) = (&host.data.alias, kind.label());
                    self.info(format!("'{alias}' ({kind}) is already open in column {}", index + 1));
                }
                index
            }
            None if new_column || self.columns.is_empty() => {
                if self.columns.len() >= self.max_columns() {
                    self.error(format!(
                        "No room for another column (at least {MIN_COLUMN_WIDTH} characters each)"
                    ));
                    return;
                }
                let index = if self.columns.is_empty() { 0 } else { self.active_column + 1 };
                self.columns.insert(index, key);
                self.zoomed = false;
                index
            }
            None => {
                self.columns[self.active_column] = key;
                self.active_column
            }
        };
        if self.sessions.get(&key) != Some(&SessionState::Alive) {
            self.request = Some(Request::OpenSession(Box::new(host), kind));
        }
        self.focus_column(index);
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
            .filter(|&i| self.host_session(self.hosts[self.entries[i].index].id).is_some())
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

    /// Shows the next (or previous) session in the active column, skipping
    /// sessions already visible in other columns.
    fn cycle_column_session(&mut self, forward: bool) {
        let Some(current) = self.active_key() else { return };
        let candidates: Vec<SessionKey> = self
            .entries
            .iter()
            .flat_map(|e| {
                let id = self.hosts[e.index].id;
                [SessionKey::ssh(id), SessionKey::sftp(id)]
            })
            .filter(|k| self.sessions.contains_key(k) && (*k == current || self.column_of(*k).is_none()))
            .collect();
        let Some(pos) = candidates.iter().position(|k| *k == current) else { return };
        let next = if forward { pos + 1 } else { pos + candidates.len() - 1 };
        self.columns[self.active_column] = candidates[next % candidates.len()];
        self.focus_column(self.active_column);
    }

    /// Alt shortcuts: columns, focus and sessions, from anywhere.
    fn on_alt_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        let in_terminal = self.focus == Focus::Terminal;
        match key.code {
            // The list is the leftmost column.
            KeyCode::Char('h') | KeyCode::Left if in_terminal && self.active_column > 0 => {
                self.focus_column(self.active_column - 1);
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.focus = Focus::List;
                self.zoomed = false;
            }
            KeyCode::Char('l') | KeyCode::Right if in_terminal => {
                self.focus_column(self.active_column + 1);
            }
            KeyCode::Char('l') | KeyCode::Right => self.focus_column(0),
            KeyCode::Char(c @ '1'..='9') => self.focus_column(usize::from(c as u8 - b'1')),
            KeyCode::Char('j') if in_terminal => self.cycle_column_session(true),
            KeyCode::Char('k') if in_terminal => self.cycle_column_session(false),
            KeyCode::Char('j') => self.switch_session(true),
            KeyCode::Char('k') => self.switch_session(false),
            KeyCode::Char('v') => self.open_in_column(SessionKind::Ssh, true),
            // History mode for the active column's session.
            KeyCode::Char('s') => {
                if let Some(key) = self.active_key().filter(|k| self.sessions.contains_key(k)) {
                    self.focus_column(self.active_column);
                    if !self.history_sessions.contains(&key) {
                        self.request = Some(Request::StartHistory(key));
                    }
                }
            }
            KeyCode::Char('w') => {
                let index = if in_terminal {
                    Some(self.active_column)
                } else {
                    let host = self.selected_host().map(|h| h.id);
                    self.columns.iter().position(|k| Some(k.host) == host)
                };
                if let Some(index) = index {
                    self.remove_column(index);
                    if in_terminal && !self.columns.is_empty() {
                        self.focus_column(self.active_column);
                    }
                }
            }
            // Full screen for the active column, inside sshh.
            KeyCode::Char('f' | 'z') if !self.columns.is_empty() => {
                self.zoomed = !self.zoomed;
                if self.zoomed {
                    self.focus_column(self.active_column);
                }
            }
            KeyCode::Char('f' | 'z') => {}
            // Alt-Shift-h / Alt-Shift-l: move the active column.
            KeyCode::Char('H') if self.active_column > 0 => {
                self.columns.swap(self.active_column, self.active_column - 1);
                self.focus_column(self.active_column - 1);
            }
            KeyCode::Char('L') if self.active_column + 1 < self.columns.len() => {
                self.columns.swap(self.active_column, self.active_column + 1);
                self.focus_column(self.active_column + 1);
            }
            KeyCode::Char('H' | 'L') => {}
            _ => return false,
        }
        true
    }

    /// Keys while a terminal column has focus: everything goes to ssh.
    fn on_terminal_key(&mut self, key: KeyEvent) {
        let Some(session) = self.active_key() else {
            self.focus = Focus::List;
            return;
        };
        if self.history_sessions.contains(&session) {
            self.request = Some(Request::HistoryKey(session, key));
            return;
        }
        match (self.sessions.get(&session), key.code) {
            (Some(SessionState::Alive), _) => self.request = Some(Request::Input(session, key)),
            (_, KeyCode::Enter | KeyCode::Char(' ')) => {
                if let Some(host) = self.host(session.host) {
                    self.request = Some(Request::OpenSession(Box::new(host.clone()), session.kind));
                }
            }
            (_, KeyCode::Esc) => self.focus = Focus::List,
            _ => {}
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::Normal if self.focus == Focus::Terminal => {
                if let Some(key) = self.active_key()
                    && self.sessions.get(&key) == Some(&SessionState::Alive)
                {
                    self.request = Some(Request::Paste(key, text.to_string()));
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
                let id = *id;
                self.mode = Mode::Normal;
                if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                    self.request = Some(Request::CloseSessions(id));
                    self.remove_columns_of_host(id);
                    self.focus = Focus::List;
                }
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
        if self.focus == Focus::List
            && self.tab == ListTab::Tags
            && self.on_tags_key(key, ctrl, pending)
        {
            return;
        }
        match key.code {
            KeyCode::Char('[' | ']') => self.switch_tab(),
            KeyCode::Char('q') => self.quit(),
            // Esc goes back one level; it never quits the app.
            KeyCode::Esc if pending.is_none() && !self.query.is_empty() => {
                self.query.clear();
                self.query_changed();
            }
            KeyCode::Char(' ') => self.connect_selected(),
            KeyCode::Enter => self.edit_selected(form::ALIAS),
            KeyCode::Char('/') => {
                self.tab = ListTab::Connections;
                self.mode = Mode::Search;
            }
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
            KeyCode::Char('s') => self.open_in_column(SessionKind::Sftp, true),
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

    /// Tab / Shift-Tab: list → details → active terminal column (if there is
    /// one) and back. Inside the terminal Tab goes to ssh.
    fn cycle_focus(&mut self, forward: bool) {
        let mut panes = vec![Focus::List, Focus::Detail];
        if !self.columns.is_empty() {
            panes.push(Focus::Terminal);
        }
        let current = panes.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward { current + 1 } else { current + panes.len() - 1 };
        match panes[next % panes.len()] {
            Focus::Terminal => self.focus_column(self.active_column),
            pane => self.focus = pane,
        }
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

    /// Closes the selected connection's sessions (asking first if one is alive).
    fn close_selected(&mut self) {
        let Some(host) = self.selected_host() else { return };
        let (id, alias) = (host.id, host.data.alias.clone());
        match self.selected_session() {
            Some(SessionState::Alive) => self.mode = Mode::ConfirmClose { id, alias },
            Some(SessionState::Ended) => {
                self.request = Some(Request::CloseSessions(id));
                self.remove_columns_of_host(id);
            }
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
            // The list is filtered while typing; Enter / Esc go back to it.
            KeyCode::Esc | KeyCode::Enter => self.mode = Mode::Normal,
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
        let column = self.column_areas.iter().find(|(_, area)| area.contains(pos)).map(|(i, _)| *i);
        let column_key = column.and_then(|i| self.columns.get(i).copied());
        let in_terminal = column_key.is_some();

        // A program that asked for the mouse (htop, vim…) gets the events over
        // its column, except with Shift (native text selection).
        if matches!(self.mode, Mode::Normal)
            && !mouse.modifiers.contains(KeyModifiers::SHIFT)
            && let (Some(index), Some(id)) = (column, column_key)
            && self.mouse_sessions.contains(&id)
            && self.sessions.get(&id) == Some(&SessionState::Alive)
        {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.focus_column(index);
            }
            let area = self.column_areas.iter().find(|(i, _)| *i == index).map(|(_, a)| *a);
            if let Some(area) = area {
                let relative = MouseEvent { column: pos.x - area.x, row: pos.y - area.y, ..mouse };
                self.request = Some(Request::Mouse(id, relative));
            }
            return;
        }
        match (&mut self.mode, mouse.kind) {
            (Mode::Form(form), MouseEventKind::Down(MouseButton::Left)) => form.on_click(pos),
            (Mode::Form(_) | Mode::ConfirmDelete { .. } | Mode::ConfirmClose { .. }, _) => {}
            (Mode::ConfirmQuit { .. }, _) => {}
            (Mode::Help, MouseEventKind::Down(_)) => self.mode = Mode::Normal,
            (Mode::Help, _) => {}
            (_, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) if let Some(id) = column_key => {
                let delta = if mouse.kind == MouseEventKind::ScrollUp { 3 } else { -3 };
                self.request = Some(Request::Scroll(id, delta));
            }
            (_, MouseEventKind::ScrollDown) if in_detail => self.scroll_detail(1),
            (_, MouseEventKind::ScrollUp) if in_detail => self.scroll_detail(-1),
            (_, MouseEventKind::ScrollDown) if self.tab == ListTab::Tags => self.move_tag_by(1),
            (_, MouseEventKind::ScrollUp) if self.tab == ListTab::Tags => self.move_tag_by(-1),
            (_, MouseEventKind::ScrollDown) => self.move_by(1),
            (_, MouseEventKind::ScrollUp) => self.move_by(-1),
            (_, MouseEventKind::Down(MouseButton::Left)) => {
                let tab = self.tab_areas.iter().find(|(_, area)| area.contains(pos)).map(|(t, _)| *t);
                if let Some(tab) = tab {
                    if tab != self.tab {
                        self.switch_tab();
                    }
                } else if self.search_area.contains(pos) {
                    self.tab = ListTab::Connections;
                    self.mode = Mode::Search;
                } else if let Some(index) = column.filter(|_| in_terminal) {
                    self.focus_column(index);
                } else if in_detail {
                    self.mode = Mode::Normal;
                    self.focus = Focus::Detail;
                } else if self.rows_area.contains(pos) {
                    let offset = match self.tab {
                        ListTab::Connections => self.table.offset(),
                        ListTab::Tags => self.tag_table.offset(),
                    };
                    self.click_row(offset + usize::from(pos.y - self.rows_area.y));
                }
            }
            _ => {}
        }
    }

    /// A click selects the row; a double click on the same row connects (or,
    /// in the Tags tab, filters by the tag).
    fn click_row(&mut self, row: usize) {
        let rows = match self.tab {
            ListTab::Connections => self.entries.len(),
            ListTab::Tags => self.tags.len(),
        };
        if row >= rows {
            return;
        }
        self.mode = Mode::Normal;
        self.focus = Focus::List;
        match self.tab {
            ListTab::Connections => self.select(row),
            ListTab::Tags => self.select_tag(row),
        }
        let double = self
            .last_click
            .is_some_and(|(r, at)| r == row && at.elapsed() < DOUBLE_CLICK);
        if double {
            self.last_click = None;
            match self.tab {
                ListTab::Connections => self.connect_selected(),
                ListTab::Tags => self.apply_tag(),
            }
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
    fn search_filters_while_typing_and_space_connects() {
        let mut app = app();
        type_query(&mut app, "#prod");
        assert_eq!(aliases(&app), ["web1", "db1"]);
        // In the search box Space is part of the query.
        app.on_key(ch(' '));
        assert_eq!(app.query, "#prod ");
        app.on_key(ctrl('j'));
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.query, "#prod ");
        assert_eq!(app.request, None);
        app.on_key(ch(' '));
        assert!(matches!(&app.request, Some(Request::OpenSession(h, SessionKind::Ssh)) if h.data.alias == "db1"));
        assert_eq!(app.focus, Focus::Terminal);
    }

    #[test]
    fn s_opens_sftp_in_a_new_column_next_to_ssh() {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        app.on_key(alt('h'));
        app.on_key(ch('s'));
        assert_eq!(app.columns, [SessionKey::ssh(2), SessionKey::sftp(2)]);
        assert_eq!((app.focus, app.active_column), (Focus::Terminal, 1));
        assert!(matches!(&app.request, Some(Request::OpenSession(h, SessionKind::Sftp)) if h.id == 2));

        // Again: it's already open, so it's just focused.
        app.sessions.insert(SessionKey::sftp(2), SessionState::Alive);
        app.request = None;
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(ch('s'));
        assert_eq!((app.columns.len(), app.active_column, &app.request), (2, 1, &None));
        // Keys go to the sftp session of the active column.
        app.on_key(ch('l'));
        assert_eq!(app.request.take(), Some(Request::Input(SessionKey::sftp(2), ch('l'))));
    }

    #[test]
    fn a_connection_is_alive_if_any_of_its_sessions_is() {
        let mut app = app();
        app.sessions.insert(SessionKey::ssh(1), SessionState::Ended);
        assert_eq!(app.host_session(1), Some(SessionState::Ended));
        app.sessions.insert(SessionKey::sftp(1), SessionState::Alive);
        assert_eq!(app.host_session(1), Some(SessionState::Alive));
        assert_eq!(app.host_session(2), None);
    }

    #[test]
    fn x_closes_every_session_of_the_connection() {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        app.on_key(alt('h'));
        app.on_key(ch('s'));
        app.sessions.insert(SessionKey::sftp(2), SessionState::Alive);
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(ch('x'));
        app.on_key(ch('y'));
        assert_eq!(app.request, Some(Request::CloseSessions(2)));
        assert!(app.columns.is_empty());
    }

    #[test]
    fn alt_j_k_also_cycle_through_sftp_sessions() {
        let mut app = app_with_sessions();
        app.sessions.insert(SessionKey::sftp(2), SessionState::Alive);
        app.on_key(ch(' '));
        app.on_key(alt('j'));
        assert_eq!(app.columns, [SessionKey::sftp(2)]);
        app.on_key(alt('j'));
        assert_eq!(app.columns, [SessionKey::ssh(3)]);
    }

    #[test]
    fn alt_s_enters_history_mode_and_routes_keys_to_it() {
        let mut app = app_with_sessions();
        app.on_key(alt('s'));
        assert_eq!(app.request, None, "no column open");
        app.on_key(ch(' '));
        app.on_key(alt('h'));
        app.on_key(alt('s'));
        assert_eq!((app.focus, app.request.take()), (Focus::Terminal, Some(Request::StartHistory(SessionKey::ssh(2)))));
        // The loop reports the session is in history mode: keys go there.
        app.history_sessions.insert(SessionKey::ssh(2));
        app.on_key(ch('k'));
        assert_eq!(app.request.take(), Some(Request::HistoryKey(SessionKey::ssh(2), ch('k'))));
        app.on_key(alt('s'));
        assert_eq!(app.request, None);
        app.history_sessions.clear();
        app.on_key(ch('k'));
        assert_eq!(app.request.take(), Some(Request::Input(SessionKey::ssh(2), ch('k'))));
    }

    #[test]
    fn mouse_goes_to_programs_that_want_it() {
        let mut app = app_with_two_columns();
        app.column_areas = vec![(0, Rect::new(30, 1, 40, 20)), (1, Rect::new(72, 1, 40, 20))];
        let at = |kind, column, modifiers| MouseEvent { kind, column, row: 5, modifiers };
        let none = KeyModifiers::NONE;
        // db1 (column 1) runs a program that wants the mouse; dev's session ended.
        app.mouse_sessions.insert(SessionKey::ssh(2));
        app.on_mouse(at(MouseEventKind::Down(MouseButton::Left), 35, none));
        assert_eq!(app.active_column, 0);
        // Relative to the column: (35, 5) in a column starting at (30, 1).
        let expected = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row: 4, modifiers: none };
        assert_eq!(app.request.take(), Some(Request::Mouse(SessionKey::ssh(2), expected)));
        app.on_mouse(at(MouseEventKind::ScrollUp, 35, none));
        assert!(matches!(app.request.take(), Some(Request::Mouse(k, _)) if k == SessionKey::ssh(2)));
        // With Shift, or without mouse mode, sshh handles it as before.
        app.on_mouse(at(MouseEventKind::ScrollUp, 35, KeyModifiers::SHIFT));
        assert_eq!(app.request.take(), Some(Request::Scroll(SessionKey::ssh(2), 3)));
        app.mouse_sessions.clear();
        app.on_mouse(at(MouseEventKind::ScrollUp, 35, none));
        assert_eq!(app.request.take(), Some(Request::Scroll(SessionKey::ssh(2), 3)));
    }

    #[test]
    fn tags_tab_lists_tags_with_counts() {
        let app = app();
        let tags: Vec<(&str, usize)> = app.tags.iter().map(|t| (t.name.as_str(), t.count)).collect();
        assert_eq!(tags, [("db", 1), ("dev", 1), ("prod", 2), ("web", 1)]);
    }

    #[test]
    fn space_on_a_tag_filters_the_connections() {
        let mut app = app();
        app.on_key(ch(']'));
        assert_eq!(app.tab, ListTab::Tags);
        app.on_key(ch('j'));
        app.on_key(ch('j'));
        assert_eq!(app.selected_tag().unwrap().name, "prod");
        app.on_key(ch(' '));
        assert_eq!((app.tab, app.query.as_str()), (ListTab::Connections, "#prod"));
        assert_eq!(aliases(&app), ["web1", "db1"]);
        assert_eq!(app.request, None);
        // Esc clears the filter as usual.
        app.on_key(key(KeyCode::Esc));
        assert_eq!(aliases(&app).len(), 3);
    }

    #[test]
    fn tags_tab_ignores_connection_actions() {
        let mut app = app();
        app.on_key(ch('['));
        for k in [ch('x'), ch('e'), ch('f'), ch('d'), ch('d'), ch('y'), ch('y')] {
            app.on_key(k);
        }
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.request, None);
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.tab, ListTab::Connections);
        // `/` from the Tags tab searches the connections.
        app.on_key(ch(']'));
        app.on_key(ch('/'));
        assert!(matches!(app.mode, Mode::Search));
        assert_eq!(app.tab, ListTab::Connections);
    }

    #[test]
    fn clicking_a_tab_switches_to_it() {
        let mut app = app();
        app.tab_areas = vec![(ListTab::Connections, Rect::new(1, 3, 18, 1)), (ListTab::Tags, Rect::new(20, 3, 8, 1))];
        let click = |column| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(click(22));
        assert_eq!(app.tab, ListTab::Tags);
        app.on_mouse(click(5));
        assert_eq!(app.tab, ListTab::Connections);
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
        for (c, program) in [('c', External::SshCopyId), ('f', External::Ssh)] {
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
        app.sessions.insert(SessionKey::ssh(2), SessionState::Alive);
        app.sessions.insert(SessionKey::ssh(3), SessionState::Ended);
        app.on_key(ch('j'));
        app
    }

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    fn columns(app: &App) -> Vec<&str> {
        app.columns.iter().map(|k| app.host(k.host).unwrap().data.alias.as_str()).collect()
    }

    /// Columns [db1, dev] with the second one active.
    fn app_with_two_columns() -> App {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        app.on_key(alt('h'));
        app.on_key(ch('j'));
        app.on_key(alt('v'));
        app.request = None;
        app
    }

    #[test]
    fn space_shows_the_selection_in_the_active_column() {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        // db1's session is alive: shown without reopening it.
        assert_eq!((columns(&app), app.focus, &app.request), (vec!["db1"], Focus::Terminal, &None));
        app.on_key(alt('h'));
        app.on_key(ch('k'));
        app.on_key(ch(' '));
        assert_eq!(columns(&app), ["web1"]);
        assert!(matches!(&app.request, Some(Request::OpenSession(h, SessionKind::Ssh)) if h.id == 1));
    }

    #[test]
    fn alt_v_splits_and_alt_h_l_navigate_columns() {
        let mut app = app_with_two_columns();
        assert_eq!(columns(&app), ["db1", "dev"]);
        assert_eq!((app.focus, app.active_column), (Focus::Terminal, 1));
        app.on_key(alt('h'));
        assert_eq!(app.active_column, 0);
        // The list selection follows the focused column.
        assert_eq!(app.selected_host().unwrap().data.alias, "db1");
        app.on_key(alt('h'));
        assert_eq!(app.focus, Focus::List);
        app.on_key(alt('l'));
        assert_eq!((app.focus, app.active_column), (Focus::Terminal, 0));
        app.on_key(alt('2'));
        assert_eq!(app.active_column, 1);
        app.on_key(alt('9'));
        assert_eq!(app.active_column, 1);
        app.on_key(alt('l'));
        assert_eq!(app.active_column, 1);
        // A connection already in a column is focused, not opened twice.
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(alt('v'));
        assert_eq!((columns(&app).len(), app.active_column), (2, 0));
    }

    #[test]
    fn terminal_focus_sends_keys_to_the_active_column() {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        for k in [ch('q'), ctrl('c'), key(KeyCode::Esc), ch('/'), key(KeyCode::Tab)] {
            app.on_key(k);
            assert_eq!(app.request.take(), Some(Request::Input(SessionKey::ssh(2), k)));
        }
        assert_eq!(app.outcome, None);
        app.on_paste("ls\n");
        assert_eq!(app.request.take(), Some(Request::Paste(SessionKey::ssh(2), "ls\n".into())));
    }

    #[test]
    fn alt_l_needs_a_column() {
        let mut app = app_with_sessions();
        app.on_key(alt('l'));
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn alt_j_k_switch_the_session_of_the_column() {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        app.on_key(alt('j'));
        assert_eq!(columns(&app), ["dev"]);
        app.on_key(alt('k'));
        assert_eq!(columns(&app), ["db1"]);
        // Sessions visible in another column are skipped.
        let mut app = app_with_two_columns();
        app.on_key(alt('j'));
        assert_eq!(columns(&app), ["db1", "dev"]);
        // In the list, Alt-j/k move the selection between sessions.
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(alt('j'));
        assert_eq!(app.selected_host().unwrap().id, 3);
    }

    #[test]
    fn alt_w_closes_columns_but_keeps_sessions() {
        let mut app = app_with_two_columns();
        app.on_key(alt('w'));
        assert_eq!((columns(&app), app.active_column, app.focus), (vec!["db1"], 0, Focus::Terminal));
        assert!(app.sessions.contains_key(&SessionKey::ssh(3)));
        assert_eq!(app.request, None);
        app.on_key(alt('w'));
        assert!(app.columns.is_empty());
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn zoom_and_move_columns() {
        let mut app = app_with_two_columns();
        app.on_key(alt('H'));
        assert_eq!((columns(&app), app.active_column), (vec!["dev", "db1"], 0));
        app.on_key(alt('L'));
        assert_eq!((columns(&app), app.active_column), (vec!["db1", "dev"], 1));
        app.on_key(alt('z'));
        assert!(app.zoomed);
        app.on_key(alt('h'));
        assert!(app.zoomed && app.active_column == 0);
        app.on_key(alt('h'));
        assert!(!app.zoomed);
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn alt_f_toggles_full_screen_and_alt_arrows_move() {
        let mut app = app_with_two_columns();
        app.on_key(alt('f'));
        assert!(app.zoomed);
        app.on_key(alt('f'));
        assert!(!app.zoomed);
        assert_eq!(app.request, None);
        let alt_key = |code| KeyEvent::new(code, KeyModifiers::ALT);
        app.on_key(alt_key(KeyCode::Left));
        assert_eq!(app.active_column, 0);
        app.on_key(alt_key(KeyCode::Right));
        assert_eq!(app.active_column, 1);
        app.on_key(alt_key(KeyCode::Left));
        app.on_key(alt_key(KeyCode::Left));
        assert_eq!(app.focus, Focus::List);
        // Alt-f in the list doesn't run the full-screen ssh of `f`.
        app.on_key(alt('f'));
        assert_eq!(app.request, None);
        assert!(app.zoomed);
    }

    #[test]
    fn splitting_respects_the_minimum_width() {
        let mut app = app_with_sessions();
        app.columns_area = Rect::new(30, 0, 90, 30);
        app.on_key(alt('v'));
        app.on_key(alt('h'));
        app.on_key(ch('j'));
        app.on_key(alt('v'));
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(ch('g'));
        app.on_key(ch('g'));
        app.on_key(alt('v'));
        assert_eq!(columns(&app), ["db1", "dev"]);
        assert!(app.status.as_ref().unwrap().error);
    }

    #[test]
    fn ended_session_reconnects_on_enter() {
        let mut app = app_with_sessions();
        app.on_key(ch('j'));
        app.on_key(ch(' '));
        app.request = None;
        app.on_key(ch('a'));
        assert_eq!(app.request, None);
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(&app.request, Some(Request::OpenSession(h, SessionKind::Ssh)) if h.id == 3));
        app.request = None;
        app.on_key(ch(' '));
        assert!(matches!(&app.request, Some(Request::OpenSession(h, SessionKind::Ssh)) if h.id == 3));
    }

    #[test]
    fn closing_and_quitting_ask_when_sessions_are_alive() {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        app.on_key(alt('h'));
        app.on_key(ch('x'));
        assert!(matches!(&app.mode, Mode::ConfirmClose { id: 2, .. }));
        app.on_key(ch('y'));
        assert_eq!(app.request.take(), Some(Request::CloseSessions(2)));
        assert!(app.columns.is_empty());

        app.on_key(ch('q'));
        assert!(matches!(app.mode, Mode::ConfirmQuit { sessions: 1 }));
        app.on_key(ch('n'));
        assert_eq!(app.outcome, None);
        app.on_key(ctrl('c'));
        app.on_key(ch('y'));
        assert_eq!(app.outcome, Some(Outcome::Quit));
    }

    #[test]
    fn mouse_on_columns() {
        let mut app = app_with_two_columns();
        app.column_areas = vec![(0, Rect::new(30, 1, 40, 20)), (1, Rect::new(72, 1, 40, 20))];
        let at = |kind, column| MouseEvent { kind, column, row: 5, modifiers: KeyModifiers::NONE };
        app.on_mouse(at(MouseEventKind::ScrollUp, 35));
        assert_eq!(app.request.take(), Some(Request::Scroll(SessionKey::ssh(2), 3)));
        app.on_mouse(at(MouseEventKind::ScrollDown, 80));
        assert_eq!(app.request.take(), Some(Request::Scroll(SessionKey::ssh(3), -3)));
        app.on_mouse(at(MouseEventKind::Down(MouseButton::Left), 35));
        assert_eq!((app.focus, app.active_column), (Focus::Terminal, 0));
    }

    #[test]
    fn yy_copies() {
        let mut app = app();
        app.on_key(ch('y'));
        app.on_key(ch('y'));
        assert!(matches!(app.request, Some(Request::Copy(ref d)) if d.alias == "web1"));
    }

    #[test]
    fn enter_and_e_edit_the_selection() {
        for k in [key(KeyCode::Enter), ch('e')] {
            let mut app = app();
            app.on_key(k);
            assert!(matches!(&app.mode, Mode::Form(f) if f.kind == FormKind::Edit(1)));
        }
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

        // With a column the terminal joins the cycle; inside it Tab goes to ssh.
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        app.on_key(alt('h'));
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.focus, Focus::Terminal);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.request.take(), Some(Request::Input(SessionKey::ssh(2), key(KeyCode::Tab))));
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
        assert!(matches!(&app.request, Some(Request::OpenSession(h, SessionKind::Ssh)) if h.id == 2));
        // Clicking outside the rows does nothing.
        app.on_mouse(click(14));
        assert_eq!(app.table.selected(), Some(1));
    }
}
