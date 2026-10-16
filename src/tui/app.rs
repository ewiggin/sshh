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
use crate::db::{HistoryEntry, Layout};
use crate::model::{Host, HostData};
use crate::theme::Theme;

/// Lines of the help (`?`): key and description; empty ones separate groups.
pub const HELP: &[(&str, &str)] = &[
    ("Space", "show the connection in the active pane"),
    ("Alt-v", "show it in a new column, right of the active one"),
    ("Alt--", "show it in a new pane below the last column"),
    ("Alt-h/j/k/l", "focus left / down / up / right (also Alt-arrows)"),
    ("Alt-1 … Alt-9", "jump to column N"),
    ("Alt-J", "stack the pane below the column on its right (or left)"),
    ("Alt-K", "take the pane out into its own column"),
    ("Alt-H / Alt-L", "move the column left / right"),
    ("Alt-n / Alt-p", "next / previous session in the pane"),
    ("Alt-w", "close the pane (the session keeps running)"),
    ("Alt-f / Alt-z", "full screen for the active pane (toggle)"),
    ("Alt-s", "history mode: j/k, / search, n/N, v select, y copy"),
    ("Alt-a", "broadcast: type in every visible pane (toggle)"),
    ("x", "close the connection's sessions (ssh and sftp)"),
    ("f", "full-screen ssh (back to sshh on exit)"),
    ("", ""),
    ("j/k ↑/↓", "move"),
    ("gg / G", "go to top / bottom"),
    ("Ctrl-d/u", "half page down / up"),
    ("Ctrl-f/b", "page down / up"),
    ("Tab / S-Tab", "next / previous pane (list, details, terminal)"),
    ("/", "search (filters while typing; #tag by tag)"),
    ("[ / ]", "tabs: connections / tags / layouts"),
    ("Space (on a tag)", "filter the connections by it"),
    ("Alt-v (on a tag)", "open all its connections in panes"),
    ("Layouts tab", "a saves the panes, Space opens, r renames, dd deletes"),
    ("Esc", "back / clear search"),
    ("? then /", "search this help: n/N next/prev match, Esc clears"),
    ("", ""),
    ("a", "add connection"),
    ("Enter / e, t", "edit / edit tags"),
    ("dd", "delete (asks for confirmation)"),
    ("yy", "copy the ssh command"),
    ("s", "sftp in a new column, next to the ssh one"),
    ("c", "install your public key (ssh-copy-id)"),
    ("o", "change sort (recent / most used / alphabetical)"),
    ("R", "reload"),
    ("T", "theme (shown while moving, Enter saves)"),
    ("q / Ctrl-c", "quit"),
    ("", ""),
    ("Click", "select · double click connects"),
    ("Wheel", "move · scroll details or session history"),
];

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Narrowest terminal column allowed when splitting.
pub const MIN_COLUMN_WIDTH: u16 = 40;
/// Shortest pane allowed when stacking panes in a column.
const MIN_PANE_HEIGHT: u16 = 6;
/// Columns reachable with Alt-1..Alt-9.
const MAX_COLUMNS: usize = 9;
/// Longest layout name accepted by the prompt.
const MAX_LAYOUT_NAME: usize = 40;

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
    /// Open a session of each of these connections.
    OpenSessions(Vec<(Host, SessionKind)>),
    /// Save a layout, replacing the one with the same name.
    SaveLayout(Layout),
    RenameLayout { from: String, to: String },
    DeleteLayout(String),
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
    /// Copy the text of a session's screen between two cells (row, column),
    /// both included, in reading order.
    CopySelection(SessionKey, (u16, u16), (u16, u16)),
    External { program: External, host: Box<Host> },
    /// Open the theme picker (the themes are read from disk).
    Themes,
    /// Save the theme in config.toml.
    SaveTheme(String),
}

/// A pane of the terminal columns: (column, row inside the column).
pub type Pane = (usize, usize);

/// Text selected with the mouse in a pane, in cells (row, column) of its
/// session screen. It stays highlighted until the next click or key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseSelection {
    pub key: SessionKey,
    pub anchor: (u16, u16),
    pub head: (u16, u16),
    /// The button was released (and the text copied).
    pub done: bool,
}

impl MouseSelection {
    /// First and last cell, in reading order.
    pub fn bounds(&self) -> ((u16, u16), (u16, u16)) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    /// Columns selected in screen row `row` (end excluded), out of `width`.
    pub fn columns(&self, row: u16, width: u16) -> Option<(u16, u16)> {
        let ((r0, c0), (r1, c1)) = self.bounds();
        if !(r0..=r1).contains(&row) {
            return None;
        }
        let from = if row == r0 { c0 } else { 0 };
        let to = if row == r1 { c1 + 1 } else { width };
        (from < to.min(width)).then_some((from, to.min(width)))
    }
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
    Help(HelpState),
    Form(Box<Form>),
    ConfirmDelete { alias: String },
    /// Typing the name to save the panes under, or the new name of the
    /// layout `rename`.
    LayoutName { name: String, rename: Option<String> },
    ConfirmDeleteLayout { name: String },
    ConfirmClose { id: i64, alias: String },
    ConfirmQuit { sessions: usize },
    Themes(ThemePicker),
}

/// The help: its scroll and its search (`/`, less style: every line stays,
/// the matches are highlighted and `n`/`N` go through them).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelpState {
    /// First visible line.
    pub scroll: usize,
    /// Visible lines (set by the render).
    pub height: usize,
    pub query: String,
    /// The search prompt is open.
    pub typing: bool,
    /// Matching lines in order, with the matched chars of the key and of the
    /// description.
    pub matches: Vec<(usize, Vec<usize>, Vec<usize>)>,
    /// Index in `matches` of the current one.
    pub current: Option<usize>,
    /// `g` pressed (for `gg`).
    pending_g: bool,
}

impl HelpState {
    pub fn current_line(&self) -> Option<usize> {
        self.current.map(|c| self.matches[c].0)
    }

    /// Matched chars of line `line`: (key, description).
    pub fn highlights(&self, line: usize) -> (&[usize], &[usize]) {
        match self.matches.iter().find(|(l, ..)| *l == line) {
            Some((_, key, desc)) => (key, desc),
            None => (&[], &[]),
        }
    }

    fn max_scroll(&self) -> usize {
        HELP.len().saturating_sub(self.height.max(1))
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    fn scroll_by(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta).min(self.max_scroll());
    }

    /// Fuzzy search, like the connection list, over each line's key and
    /// description; the current match is the first one from the top of the view.
    fn search(&mut self, matcher: &mut Matcher) {
        self.matches.clear();
        self.current = None;
        if self.query.trim().is_empty() {
            return;
        }
        let pattern = Pattern::parse(&self.query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut indices = Vec::new();
        for (line, (key, desc)) in HELP.iter().enumerate() {
            if key.is_empty() && desc.is_empty() {
                continue;
            }
            let haystack = format!("{key} {desc}");
            indices.clear();
            if pattern.indices(Utf32Str::new(&haystack, &mut buf), matcher, &mut indices).is_none() {
                continue;
            }
            indices.sort_unstable();
            indices.dedup();
            let key_len = key.chars().count();
            let positions = indices.iter().map(|&i| i as usize);
            let key_hl = positions.clone().filter(|&i| i < key_len).collect();
            let desc_hl = positions.filter(|&i| i > key_len).map(|i| i - key_len - 1).collect();
            self.matches.push((line, key_hl, desc_hl));
        }
        let from = self.scroll;
        self.current = self.matches.iter().position(|(l, ..)| *l >= from).or((!self.matches.is_empty()).then_some(0));
        self.show_current();
    }

    /// `n` / `N`: next / previous match, wrapping around.
    fn next_match(&mut self, forward: bool) {
        let n = self.matches.len();
        if let Some(c) = self.current.filter(|_| n > 0) {
            self.current = Some(if forward { (c + 1) % n } else { (c + n - 1) % n });
            self.show_current();
        }
    }

    /// Scrolls so the current match is visible.
    fn show_current(&mut self) {
        let Some(line) = self.current_line() else { return };
        let height = self.height.max(1);
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll + height {
            self.scroll = line + 1 - height;
        }
    }
}

/// Theme picker (`T`): the highlighted theme is shown while moving; Esc goes
/// back to the one in use before.
pub struct ThemePicker {
    /// Each theme and whether it comes from the user's themes folder.
    pub themes: Vec<(Theme, bool)>,
    pub selected: usize,
    pub previous: Theme,
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
    Layouts,
}

impl ListTab {
    pub const ALL: [Self; 3] = [Self::Connections, Self::Tags, Self::Layouts];
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
    /// Saved layouts, by name, for the Layouts tab (loaded by the main loop).
    pub layouts: Vec<Layout>,
    pub layout_table: TableState,
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
    /// Text being (or just) selected with the mouse in a pane.
    pub selection: Option<MouseSelection>,
    /// Terminal columns, left to right; each one is a stack of panes (sessions),
    /// top to bottom.
    pub columns: Vec<Vec<SessionKey>>,
    pub active_column: usize,
    /// Active pane inside the active column.
    pub active_row: usize,
    /// The active column takes the whole screen.
    pub zoomed: bool,
    /// Keys typed in a pane also go to the other visible panes (Alt-a).
    pub broadcast: bool,
    /// Areas from the last render (for the mouse and to size sessions).
    pub rows_area: Rect,
    pub search_area: Rect,
    pub detail_area: Rect,
    /// Whole area for the terminal columns.
    pub columns_area: Rect,
    /// Widest the columns can get, with the list at its minimum width (0
    /// until the first render, or in portrait mode).
    pub columns_room: u16,
    /// Inner area of every visible pane, by (column, row).
    pub pane_areas: Vec<(Pane, Rect)>,
    /// Narrow screen: the list on top and the panes below it, one under the
    /// other (set by the render).
    pub portrait: bool,
    /// The panes are drawn one under the other (portrait, or the columns
    /// don't fit side by side), so Alt-j/k go through all of them.
    pub stacked: bool,
    /// Colors applied after drawing (loaded by the main loop).
    pub theme: Theme,
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
            layouts: Vec::new(),
            layout_table: TableState::default(),
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
            selection: None,
            columns: Vec::new(),
            active_column: 0,
            active_row: 0,
            zoomed: false,
            broadcast: false,
            rows_area: Rect::default(),
            search_area: Rect::default(),
            detail_area: Rect::default(),
            columns_area: Rect::default(),
            columns_room: 0,
            portrait: false,
            stacked: false,
            theme: Theme::terminal(),
            pane_areas: Vec::new(),
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

    /// Session shown in the active pane.
    pub fn active_key(&self) -> Option<SessionKey> {
        self.columns.get(self.active_column)?.get(self.active_row).copied()
    }

    /// Sessions that get what is typed in the pane of `key`: only that one,
    /// or with broadcast on, also every other visible pane with a live session
    /// of the same kind (not in history mode).
    pub fn input_targets(&self, key: SessionKey) -> Vec<SessionKey> {
        let mut targets = vec![key];
        if self.broadcast && !self.zoomed {
            targets.extend(self.columns.iter().flatten().copied().filter(|k| {
                *k != key
                    && k.kind == key.kind
                    && self.sessions.get(k) == Some(&SessionState::Alive)
                    && !self.history_sessions.contains(k)
            }));
        }
        targets
    }

    /// Pane showing session `key`, if any.
    fn pane_of(&self, key: SessionKey) -> Option<Pane> {
        self.columns
            .iter()
            .enumerate()
            .find_map(|(c, column)| column.iter().position(|k| *k == key).map(|r| (c, r)))
    }

    /// Focuses pane (`column`, `row`), clamping the row to the column, and
    /// selects its connection in the list.
    fn focus_pane(&mut self, column: usize, row: usize) {
        let Some(panes) = self.columns.get(column) else { return };
        let row = row.min(panes.len() - 1);
        let key = panes[row];
        self.active_column = column;
        self.active_row = row;
        self.mode = Mode::Normal;
        self.focus = Focus::Terminal;
        if let Some(entry) = self.entries.iter().position(|e| self.hosts[e.index].id == key.host) {
            self.select(entry);
        }
    }

    /// With the panes one under the other: focuses the next (or previous) one,
    /// in order, column by column.
    fn focus_stacked(&mut self, forward: bool) {
        let panes: Vec<Pane> =
            self.columns.iter().enumerate().flat_map(|(c, panes)| (0..panes.len()).map(move |r| (c, r))).collect();
        let Some(index) = panes.iter().position(|&p| p == (self.active_column, self.active_row)) else { return };
        let next = if forward { (index + 1).min(panes.len() - 1) } else { index.saturating_sub(1) };
        let (column, row) = panes[next];
        self.focus_pane(column, row);
    }

    /// Focuses column `index`, keeping the active row if it has it.
    fn focus_column(&mut self, index: usize) {
        self.focus_pane(index, self.active_row);
    }

    /// Removes the pane showing session `key`, if any.
    pub fn remove_column_of(&mut self, key: SessionKey) {
        if let Some(pane) = self.pane_of(key) {
            self.remove_pane(pane);
        }
    }

    /// Removes every pane of connection `host`.
    pub fn remove_columns_of_host(&mut self, host: i64) {
        while let Some(pane) = self.columns.iter().enumerate().find_map(|(c, column)| {
            column.iter().position(|k| k.host == host).map(|r| (c, r))
        }) {
            self.remove_pane(pane);
        }
    }

    /// Removes a pane; the rest of its column (or the other columns) take the
    /// space.
    fn remove_pane(&mut self, (column, row): Pane) {
        let Some(panes) = self.columns.get_mut(column) else { return };
        if row >= panes.len() {
            return;
        }
        panes.remove(row);
        if panes.is_empty() {
            self.columns.remove(column);
            if column < self.active_column {
                self.active_column -= 1;
            }
        } else if column == self.active_column && row < self.active_row {
            self.active_row -= 1;
        }
        if self.columns.is_empty() {
            self.active_column = 0;
            self.active_row = 0;
            self.zoomed = false;
            self.broadcast = false;
            if self.focus == Focus::Terminal {
                self.focus = Focus::List;
            }
        } else {
            self.active_column = self.active_column.min(self.columns.len() - 1);
            self.active_row = self.active_row.min(self.columns[self.active_column].len() - 1);
        }
    }

    fn max_columns(&self) -> usize {
        // The list gets narrower as columns are added.
        match self.columns_room.max(self.columns_area.width) {
            0 => MAX_COLUMNS,
            width => usize::from(width / MIN_COLUMN_WIDTH).clamp(1, MAX_COLUMNS),
        }
    }

    fn max_panes_per_column(&self) -> usize {
        match self.columns_area.height {
            0 => MAX_COLUMNS,
            height => usize::from(height / MIN_PANE_HEIGHT).max(1),
        }
    }

    /// In portrait mode every pane goes under the previous one: how many fit.
    fn stacked_capacity(&self) -> Option<usize> {
        self.portrait.then(|| match self.columns_area.height {
            0 => MAX_COLUMNS,
            height => usize::from(height / MIN_PANE_HEIGHT).clamp(1, MAX_COLUMNS),
        })
    }

    fn total_panes(&self) -> usize {
        self.columns.iter().map(Vec::len).sum()
    }

    fn room_for_column(&self) -> bool {
        match self.stacked_capacity() {
            Some(capacity) => self.total_panes() < capacity,
            None => self.columns.len() < self.max_columns(),
        }
    }

    fn room_for_pane(&self, column: usize) -> bool {
        match self.stacked_capacity() {
            Some(capacity) => self.total_panes() < capacity,
            None => self.columns[column].len() < self.max_panes_per_column(),
        }
    }

    fn no_room_for_columns(&mut self) {
        if self.portrait {
            self.no_room_for_panes();
        } else {
            self.error(format!("No room for another column (at least {MIN_COLUMN_WIDTH} characters each)"));
        }
    }

    fn no_room_for_panes(&mut self) {
        self.error(format!("No room for another pane (at least {MIN_PANE_HEIGHT} lines each)"));
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
            self.columns.iter().flatten().map(|k| k.host).filter(|id| self.host(*id).is_none()).collect();
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

    /// Opens the theme picker on the theme in use (`themes` is never empty:
    /// it has at least `terminal`).
    pub fn open_themes(&mut self, themes: Vec<(Theme, bool)>) {
        let selected = themes.iter().position(|(t, _)| t.name == self.theme.name).unwrap_or(0);
        self.mode = Mode::Themes(ThemePicker { themes, selected, previous: self.theme.clone() });
    }

    /// Leaves the theme picker without saving.
    fn close_themes(&mut self) {
        if let Mode::Themes(picker) = std::mem::replace(&mut self.mode, Mode::Normal) {
            self.theme = picker.previous;
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

    /// Replaces the layouts (after saving or deleting one), selecting `name`
    /// or, failing that, the same position.
    pub fn set_layouts(&mut self, layouts: Vec<Layout>, name: Option<&str>) {
        let index = name.and_then(|name| layouts.iter().position(|l| l.name == name));
        self.layouts = layouts;
        self.select_layout(index.unwrap_or_else(|| self.layout_table.selected().unwrap_or(0)));
    }

    pub fn selected_layout(&self) -> Option<&Layout> {
        self.layouts.get(self.layout_table.selected()?)
    }

    fn select_layout(&mut self, index: usize) {
        let selected = self.layouts.len().checked_sub(1).map(|last| index.min(last));
        if selected != self.layout_table.selected() {
            self.detail_scroll = 0;
        }
        self.layout_table.select(selected);
    }

    /// Selects a row of the Tags or Layouts tab.
    fn select_side(&mut self, index: usize) {
        match self.tab {
            ListTab::Layouts => self.select_layout(index),
            _ => self.select_tag(index),
        }
    }

    fn move_side_by(&mut self, delta: isize) {
        let table = if self.tab == ListTab::Layouts { &self.layout_table } else { &self.tag_table };
        let current = table.selected().unwrap_or(0) as isize;
        self.select_side(current.saturating_add(delta).max(0) as usize);
    }

    fn set_tab(&mut self, tab: ListTab) {
        self.tab = tab;
        self.mode = Mode::Normal;
        self.focus = Focus::List;
        self.detail_scroll = 0;
    }

    fn switch_tab(&mut self, forward: bool) {
        let tabs = ListTab::ALL;
        let current = tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
        let next = if forward { current + 1 } else { current + tabs.len() - 1 };
        self.set_tab(tabs[next % tabs.len()]);
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

    /// `a` in the Layouts tab: asks for a name to save the panes under.
    fn name_layout(&mut self) {
        if self.columns.is_empty() {
            self.error("No panes to save: open some connections first");
        } else {
            self.mode = Mode::LayoutName { name: String::new(), rename: None };
        }
    }

    /// `r` in the Layouts tab: asks for a new name for the selected layout.
    fn rename_layout(&mut self) {
        if let Some(layout) = self.selected_layout() {
            let name = layout.name.clone();
            self.mode = Mode::LayoutName { name: name.clone(), rename: Some(name) };
        }
    }

    /// Space on a layout: replaces the panes with the saved ones, opening the
    /// sessions that aren't alive. The sessions of the panes it replaces keep
    /// running.
    fn open_layout(&mut self) {
        let Some(layout) = self.selected_layout() else { return };
        let name = layout.name.clone();
        let mut columns: Vec<Vec<SessionKey>> = layout
            .columns
            .iter()
            .map(|panes| {
                let keys = panes.iter().map(|&(host, sftp)| {
                    SessionKey { host, kind: if sftp { SessionKind::Sftp } else { SessionKind::Ssh } }
                });
                keys.filter(|k| self.host(k.host).is_some()).collect::<Vec<_>>()
            })
            .filter(|panes| !panes.is_empty())
            .collect();
        let total: usize = columns.iter().map(Vec::len).sum();
        if total == 0 {
            self.error(format!("Layout '{name}' has no connections left"));
            return;
        }
        let max_panes = self.max_panes_per_column();
        let fits = match self.stacked_capacity() {
            Some(capacity) => total <= capacity,
            None => columns.len() <= self.max_columns() && columns.iter().all(|panes| panes.len() <= max_panes),
        };
        if !fits {
            columns = self.grid(columns.into_iter().flatten().collect());
        }
        let shown: usize = columns.iter().map(Vec::len).sum();
        self.show_panes(columns);
        if shown < total {
            self.error(format!("Layout '{name}': only {shown} of its {total} panes fit"));
        } else if !fits {
            self.info(format!("Layout '{name}' open, rearranged to fit"));
        } else {
            self.info(format!("Layout '{name}' open"));
        }
    }

    /// Keys of the Tags and Layouts tabs; returns false for the general keys
    /// they let through.
    fn on_side_key(&mut self, key: KeyEvent, ctrl: bool, pending: Option<char>) -> bool {
        let page = self.page();
        let layouts = self.tab == ListTab::Layouts;
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter if layouts => self.open_layout(),
            KeyCode::Char(' ') | KeyCode::Enter => self.apply_tag(),
            KeyCode::Esc => self.tab = ListTab::Connections,
            KeyCode::Char('d') if ctrl => self.move_side_by(page / 2),
            KeyCode::Char('u') if ctrl => self.move_side_by(-page / 2),
            KeyCode::Char('f') if ctrl => self.move_side_by(page),
            KeyCode::Char('b') if ctrl => self.move_side_by(-page),
            KeyCode::Char('j') | KeyCode::Down => self.move_side_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_side_by(-1),
            KeyCode::Char('G') | KeyCode::End => self.move_side_by(isize::MAX),
            KeyCode::Home => self.select_side(0),
            KeyCode::PageDown => self.move_side_by(page),
            KeyCode::PageUp => self.move_side_by(-page),
            KeyCode::Char('g') if pending == Some('g') => self.select_side(0),
            KeyCode::Char('g') => self.pending = Some('g'),
            KeyCode::Char('a') if layouts => self.name_layout(),
            KeyCode::Char('r') if layouts => self.rename_layout(),
            KeyCode::Char('d') if layouts && pending == Some('d') => {
                if let Some(layout) = self.selected_layout() {
                    self.mode = Mode::ConfirmDeleteLayout { name: layout.name.clone() };
                }
            }
            KeyCode::Char('d') if layouts => self.pending = Some('d'),
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

    /// Shows a session of the selected connection in the active pane or, with
    /// `new_column`, in a new column right of the active one. Opens the session
    /// if needed. A session already shown in a pane just gets focused.
    fn open_in_column(&mut self, kind: SessionKind, new_column: bool) {
        let Some(host) = self.selected_host().cloned() else { return };
        let key = SessionKey { host: host.id, kind };
        let (column, row) = match self.pane_of(key) {
            Some(pane) => {
                if new_column {
                    self.already_open(&host, kind, pane);
                }
                pane
            }
            None if new_column || self.columns.is_empty() => {
                if !self.room_for_column() {
                    self.no_room_for_columns();
                    return;
                }
                let index = if self.columns.is_empty() { 0 } else { self.active_column + 1 };
                self.columns.insert(index, vec![key]);
                self.zoomed = false;
                (index, 0)
            }
            None => {
                self.columns[self.active_column][self.active_row] = key;
                (self.active_column, self.active_row)
            }
        };
        if self.sessions.get(&key) != Some(&SessionState::Alive) {
            self.request = Some(Request::OpenSession(Box::new(host), kind));
        }
        self.focus_pane(column, row);
    }

    /// Alt--: shows the selected connection in a new pane below the last column.
    fn open_below(&mut self) {
        if self.columns.is_empty() {
            self.open_in_column(SessionKind::Ssh, true);
            return;
        }
        let Some(host) = self.selected_host().cloned() else { return };
        let key = SessionKey::ssh(host.id);
        if let Some(pane) = self.pane_of(key) {
            self.already_open(&host, SessionKind::Ssh, pane);
            self.focus_pane(pane.0, pane.1);
            return;
        }
        let last = self.columns.len() - 1;
        if !self.room_for_pane(last) {
            self.no_room_for_panes();
            return;
        }
        self.columns[last].push(key);
        self.zoomed = false;
        if self.sessions.get(&key) != Some(&SessionState::Alive) {
            self.request = Some(Request::OpenSession(Box::new(host), SessionKind::Ssh));
        }
        self.focus_pane(last, self.columns[last].len() - 1);
    }

    /// Alt-v in the Tags tab: replaces the panes with one per connection of the
    /// highlighted tag, in a grid, opening the sessions that aren't alive. The
    /// sessions of the panes it replaces keep running.
    fn open_tag(&mut self) {
        let Some(tag) = self.selected_tag().map(|t| t.name.clone()) else { return };
        let keys: Vec<SessionKey> =
            self.hosts.iter().filter(|h| h.data.tags.contains(&tag)).map(|h| SessionKey::ssh(h.id)).collect();
        let total = keys.len();
        let columns = self.grid(keys);
        let shown: usize = columns.iter().map(Vec::len).sum();
        if shown == 0 {
            return;
        }
        self.show_panes(columns);
        if shown < total {
            self.error(format!("'#{tag}' has {total} connections: only {shown} fit in the panes"));
        } else {
            self.info(format!("'#{tag}': {total} connection(s) open (Alt-a types in all of them)"));
        }
    }

    /// Arranges sessions in as many columns as fit, stacking the rest evenly.
    /// The ones that don't fit are left out.
    /// In portrait mode, as many as fit, one per column (they are drawn one
    /// under the other anyway).
    fn grid(&self, mut keys: Vec<SessionKey>) -> Vec<Vec<SessionKey>> {
        if let Some(capacity) = self.stacked_capacity() {
            keys.truncate(capacity);
            return keys.into_iter().map(|key| vec![key]).collect();
        }
        let max_columns = self.max_columns();
        keys.truncate(max_columns * self.max_panes_per_column());
        let (total, count) = (keys.len(), keys.len().min(max_columns));
        let mut keys = keys.into_iter();
        (0..count).map(|c| keys.by_ref().take(total / count + usize::from(c < total % count)).collect()).collect()
    }

    /// Replaces the panes with `columns` (not empty), focusing the first one
    /// and asking for the sessions that aren't alive.
    fn show_panes(&mut self, columns: Vec<Vec<SessionKey>>) {
        self.columns = columns;
        self.zoomed = false;
        self.focus_pane(0, 0);
        let missing: Vec<(Host, SessionKind)> = self
            .columns
            .iter()
            .flatten()
            .filter(|key| self.sessions.get(key) != Some(&SessionState::Alive))
            .filter_map(|key| Some((self.host(key.host)?.clone(), key.kind)))
            .collect();
        if !missing.is_empty() {
            self.request = Some(Request::OpenSessions(missing));
        }
    }

    fn already_open(&mut self, host: &Host, kind: SessionKind, (column, row): Pane) {
        let (alias, kind) = (&host.data.alias, kind.label());
        let place = match self.columns[column].len() {
            1 => format!("column {}", column + 1),
            _ => format!("column {}, pane {}", column + 1, row + 1),
        };
        self.info(format!("'{alias}' ({kind}) is already open in {place}"));
    }

    /// Alt-Shift-j: moves the active pane below the column on its right (or on
    /// its left, if it's the last one).
    fn stack_pane(&mut self) {
        let Some(key) = self.active_key() else { return };
        let (column, row) = (self.active_column, self.active_row);
        let mut target = if column + 1 < self.columns.len() {
            column + 1
        } else if column > 0 {
            column - 1
        } else {
            return;
        };
        // Moving a pane doesn't change how many there are, which is what
        // limits them in portrait mode.
        if !self.portrait && self.columns[target].len() >= self.max_panes_per_column() {
            self.no_room_for_panes();
            return;
        }
        self.columns[column].remove(row);
        if self.columns[column].is_empty() {
            self.columns.remove(column);
            if target > column {
                target -= 1;
            }
        }
        self.columns[target].push(key);
        self.focus_pane(target, self.columns[target].len() - 1);
    }

    /// Alt-Shift-k: takes the active pane out of its stack into its own column,
    /// right of it.
    fn unstack_pane(&mut self) {
        let (column, row) = (self.active_column, self.active_row);
        if self.columns.get(column).is_none_or(|panes| panes.len() < 2) {
            return;
        }
        let room = if self.portrait { self.columns.len() < MAX_COLUMNS } else { self.columns.len() < self.max_columns() };
        if !room {
            self.no_room_for_columns();
            return;
        }
        let key = self.columns[column].remove(row);
        self.columns.insert(column + 1, vec![key]);
        self.focus_pane(column + 1, 0);
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

    /// Shows the next (or previous) session in the active pane, skipping
    /// sessions already visible in other panes.
    fn cycle_column_session(&mut self, forward: bool) {
        let Some(current) = self.active_key() else { return };
        let candidates: Vec<SessionKey> = self
            .entries
            .iter()
            .flat_map(|e| {
                let id = self.hosts[e.index].id;
                [SessionKey::ssh(id), SessionKey::sftp(id)]
            })
            .filter(|k| self.sessions.contains_key(k) && (*k == current || self.pane_of(*k).is_none()))
            .collect();
        let Some(pos) = candidates.iter().position(|k| *k == current) else { return };
        let next = if forward { pos + 1 } else { pos + candidates.len() - 1 };
        self.columns[self.active_column][self.active_row] = candidates[next % candidates.len()];
        self.focus_pane(self.active_column, self.active_row);
    }

    /// Alt shortcuts: panes, focus and sessions, from anywhere.
    fn on_alt_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        let in_terminal = self.focus == Focus::Terminal;
        let (column, row) = (self.active_column, self.active_row);
        match key.code {
            // Panes one under the other: j/k (and l) go through all of them,
            // h back to the list.
            KeyCode::Char('j' | 'l') | KeyCode::Down | KeyCode::Right if in_terminal && self.stacked => {
                self.focus_stacked(true);
            }
            KeyCode::Char('k') | KeyCode::Up if in_terminal && self.stacked => self.focus_stacked(false),
            KeyCode::Char('h') | KeyCode::Left if in_terminal && self.stacked => {
                self.focus = Focus::List;
                self.zoomed = false;
            }
            // Focus, like vim; the list is the leftmost column.
            KeyCode::Char('h') | KeyCode::Left if in_terminal && column > 0 => {
                self.focus_pane(column - 1, row);
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.focus = Focus::List;
                self.zoomed = false;
            }
            KeyCode::Char('l') | KeyCode::Right if in_terminal => self.focus_pane(column + 1, row),
            KeyCode::Char('l') | KeyCode::Right => self.focus_pane(0, row),
            KeyCode::Char('j') | KeyCode::Down if in_terminal => self.focus_pane(column, row + 1),
            KeyCode::Char('k') | KeyCode::Up if in_terminal => {
                self.focus_pane(column, row.saturating_sub(1));
            }
            // In the list: the next / previous connection with a session.
            KeyCode::Char('j') | KeyCode::Down => self.switch_session(true),
            KeyCode::Char('k') | KeyCode::Up => self.switch_session(false),
            KeyCode::Char(c @ '1'..='9') => self.focus_column(usize::from(c as u8 - b'1')),
            // The session shown in the active pane.
            KeyCode::Char('n') => self.cycle_column_session(true),
            KeyCode::Char('p') => self.cycle_column_session(false),
            // In the Tags tab: every connection of the tag, in panes.
            KeyCode::Char('v') if !in_terminal && self.tab == ListTab::Tags => self.open_tag(),
            // Splits: a column on the right, or a pane below the last column.
            KeyCode::Char('v') => self.open_in_column(SessionKind::Ssh, true),
            KeyCode::Char('-') => self.open_below(),
            // History mode for the active pane's session.
            KeyCode::Char('s') => {
                if let Some(key) = self.active_key().filter(|k| self.sessions.contains_key(k)) {
                    self.focus_pane(column, row);
                    if !self.history_sessions.contains(&key) {
                        self.request = Some(Request::StartHistory(key));
                    }
                }
            }
            KeyCode::Char('w') => {
                let pane = if in_terminal {
                    Some((column, row))
                } else {
                    let host = self.selected_host().map(|h| h.id);
                    self.columns.iter().enumerate().find_map(|(c, panes)| {
                        panes.iter().position(|k| Some(k.host) == host).map(|r| (c, r))
                    })
                };
                if let Some(pane) = pane {
                    self.remove_pane(pane);
                    if in_terminal && !self.columns.is_empty() {
                        self.focus_pane(self.active_column, self.active_row);
                    }
                }
            }
            // Full screen for the active pane, inside sshh.
            KeyCode::Char('f' | 'z') if !self.columns.is_empty() => {
                self.zoomed = !self.zoomed;
                if self.zoomed {
                    self.focus_pane(column, row);
                }
            }
            KeyCode::Char('f' | 'z') => {}
            // Broadcast: what is typed in a pane goes to every visible one.
            KeyCode::Char('a') if self.broadcast => {
                self.broadcast = false;
                self.info("Broadcast off");
            }
            KeyCode::Char('a') if self.columns.is_empty() => self.error("Broadcast needs an open pane"),
            KeyCode::Char('a') => {
                self.broadcast = true;
                self.info("Broadcast on: keys go to every visible pane (Alt-a stops it)");
            }
            // With Shift, move: H/L the column, J stacks the pane, K unstacks it.
            KeyCode::Char('H') if in_terminal && column > 0 => {
                self.columns.swap(column, column - 1);
                self.focus_pane(column - 1, row);
            }
            KeyCode::Char('L') if in_terminal && column + 1 < self.columns.len() => {
                self.columns.swap(column, column + 1);
                self.focus_pane(column + 1, row);
            }
            KeyCode::Char('J') if in_terminal => self.stack_pane(),
            KeyCode::Char('K') if in_terminal => self.unstack_pane(),
            KeyCode::Char('H' | 'L' | 'J' | 'K') => {}
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
            Mode::LayoutName { name, .. } => {
                let room = MAX_LAYOUT_NAME.saturating_sub(name.chars().count());
                name.extend(text.chars().filter(|c| !c.is_control()).take(room));
            }
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
        self.selection = None;
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
            Mode::LayoutName { name, rename } => match key.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
                KeyCode::Enter if !name.trim().is_empty() && rename.is_some() => {
                    let (from, to) = (rename.take().unwrap_or_default(), name.trim().to_string());
                    if from != to {
                        self.request = Some(Request::RenameLayout { from, to });
                    }
                    self.mode = Mode::Normal;
                }
                KeyCode::Enter if !name.trim().is_empty() => {
                    let columns = self
                        .columns
                        .iter()
                        .map(|panes| panes.iter().map(|k| (k.host, k.kind == SessionKind::Sftp)).collect())
                        .collect();
                    self.request = Some(Request::SaveLayout(Layout { name: name.trim().to_string(), columns }));
                    self.mode = Mode::Normal;
                }
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Char(c)
                    if !ctrl
                        && !key.modifiers.contains(KeyModifiers::ALT)
                        && name.chars().count() < MAX_LAYOUT_NAME =>
                {
                    name.push(c);
                }
                _ => {}
            },
            Mode::ConfirmDeleteLayout { name } => {
                if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                    self.request = Some(Request::DeleteLayout(name.clone()));
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
            Mode::Themes(picker) => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    picker.selected = (picker.selected + 1) % picker.themes.len();
                    self.theme = picker.themes[picker.selected].0.clone();
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    picker.selected = (picker.selected + picker.themes.len() - 1) % picker.themes.len();
                    self.theme = picker.themes[picker.selected].0.clone();
                }
                KeyCode::Enter => {
                    self.request = Some(Request::SaveTheme(self.theme.name.clone()));
                    self.mode = Mode::Normal;
                }
                KeyCode::Esc | KeyCode::Char('q') => self.close_themes(),
                KeyCode::Char('c') if ctrl => self.close_themes(),
                _ => {}
            },
            Mode::ConfirmQuit { .. } => {
                if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                    self.outcome = Some(Outcome::Quit);
                }
                self.mode = Mode::Normal;
            }
            _ if ctrl && key.code == KeyCode::Char('c') => self.quit(),
            Mode::Help(_) => self.on_help_key(key, ctrl),
            Mode::Search => self.on_search_key(key, ctrl),
            Mode::Normal => self.on_normal_key(key, ctrl),
        }
    }

    /// Keys in the help: scroll, search (`/`, `n`/`N`) and close.
    fn on_help_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Mode::Help(help) = &mut self.mode else { return };
        if help.typing {
            match key.code {
                KeyCode::Esc => {
                    help.typing = false;
                    help.query.clear();
                    help.search(&mut self.matcher);
                }
                KeyCode::Enter => help.typing = false,
                KeyCode::Backspace => {
                    help.query.pop();
                    help.search(&mut self.matcher);
                }
                KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                    help.query.push(c);
                    help.search(&mut self.matcher);
                }
                _ => {}
            }
            return;
        }
        let pending_g = std::mem::take(&mut help.pending_g);
        let half = (help.height / 2).max(1) as isize;
        match key.code {
            KeyCode::Esc if !help.query.is_empty() => {
                help.query.clear();
                help.search(&mut self.matcher);
            }
            KeyCode::Esc | KeyCode::Char('q' | '?') => self.mode = Mode::Normal,
            KeyCode::Char('/') => {
                help.typing = true;
                help.query.clear();
                help.search(&mut self.matcher);
            }
            KeyCode::Char('n') if !ctrl => help.next_match(true),
            KeyCode::Char('N') => help.next_match(false),
            KeyCode::Char('d') if ctrl => help.scroll_by(half),
            KeyCode::Char('u') if ctrl => help.scroll_by(-half),
            KeyCode::Char('j') | KeyCode::Down => help.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => help.scroll_by(-1),
            KeyCode::PageDown => help.scroll_by(help.height as isize),
            KeyCode::PageUp => help.scroll_by(-(help.height as isize)),
            KeyCode::Char('G') | KeyCode::End => help.scroll = help.max_scroll(),
            KeyCode::Home => help.scroll = 0,
            KeyCode::Char('g') if pending_g => help.scroll = 0,
            KeyCode::Char('g') => help.pending_g = true,
            _ => {}
        }
    }

    fn on_normal_key(&mut self, key: KeyEvent, ctrl: bool) {
        let pending = self.pending.take();
        if self.focus == Focus::Detail && self.on_detail_key(key, ctrl) {
            return;
        }
        if self.focus == Focus::List
            && self.tab != ListTab::Connections
            && self.on_side_key(key, ctrl, pending)
        {
            return;
        }
        match key.code {
            KeyCode::Char('[') => self.switch_tab(false),
            KeyCode::Char(']') => self.switch_tab(true),
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
            KeyCode::Char('?') => self.mode = Mode::Help(HelpState::default()),
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
            KeyCode::Char('T') => self.request = Some(Request::Themes),
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
            Focus::Terminal => self.focus_pane(self.active_column, self.active_row),
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
        let pane = self.pane_areas.iter().find(|(_, area)| area.contains(pos)).copied();
        let column_key = pane.and_then(|((c, r), _)| self.columns.get(c)?.get(r).copied());
        let in_terminal = column_key.is_some();

        // A program that asked for the mouse (htop, vim…) gets the events over
        // its column, except with Shift (native text selection).
        if matches!(self.mode, Mode::Normal)
            && !mouse.modifiers.contains(KeyModifiers::SHIFT)
            && let (Some(((column, row), area)), Some(id)) = (pane, column_key)
            && self.mouse_sessions.contains(&id)
            && self.sessions.get(&id) == Some(&SessionState::Alive)
        {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.focus_pane(column, row);
            }
            let relative = MouseEvent { column: pos.x - area.x, row: pos.y - area.y, ..mouse };
            self.request = Some(Request::Mouse(id, relative));
            return;
        }
        if self.on_selection_mouse(mouse, pane.zip(column_key)) {
            return;
        }
        match (&mut self.mode, mouse.kind) {
            (Mode::Form(form), MouseEventKind::Down(MouseButton::Left)) => form.on_click(pos),
            (Mode::Form(_) | Mode::ConfirmDelete { .. } | Mode::ConfirmClose { .. }, _) => {}
            (Mode::ConfirmQuit { .. }, _) => {}
            (Mode::Help(_), MouseEventKind::Down(_)) => self.mode = Mode::Normal,
            (Mode::Help(help), MouseEventKind::ScrollDown) => help.scroll_by(3),
            (Mode::Help(help), MouseEventKind::ScrollUp) => help.scroll_by(-3),
            (Mode::Help(_), _) => {}
            (_, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) if let Some(id) = column_key => {
                let delta = if mouse.kind == MouseEventKind::ScrollUp { 3 } else { -3 };
                self.request = Some(Request::Scroll(id, delta));
            }
            (_, MouseEventKind::ScrollDown) if in_detail => self.scroll_detail(1),
            (_, MouseEventKind::ScrollUp) if in_detail => self.scroll_detail(-1),
            (_, MouseEventKind::ScrollDown) if self.tab != ListTab::Connections => self.move_side_by(1),
            (_, MouseEventKind::ScrollUp) if self.tab != ListTab::Connections => self.move_side_by(-1),
            (_, MouseEventKind::ScrollDown) => self.move_by(1),
            (_, MouseEventKind::ScrollUp) => self.move_by(-1),
            (_, MouseEventKind::Down(MouseButton::Left)) => {
                let tab = self.tab_areas.iter().find(|(_, area)| area.contains(pos)).map(|(t, _)| *t);
                if let Some(tab) = tab {
                    if tab != self.tab {
                        self.set_tab(tab);
                    }
                } else if self.search_area.contains(pos) {
                    self.tab = ListTab::Connections;
                    self.mode = Mode::Search;
                } else if let Some(((column, row), _)) = pane.filter(|_| in_terminal) {
                    self.focus_pane(column, row);
                } else if in_detail {
                    self.mode = Mode::Normal;
                    self.focus = Focus::Detail;
                } else if self.rows_area.contains(pos) {
                    let offset = match self.tab {
                        ListTab::Connections => self.table.offset(),
                        ListTab::Tags => self.tag_table.offset(),
                        ListTab::Layouts => self.layout_table.offset(),
                    };
                    self.click_row(offset + usize::from(pos.y - self.rows_area.y));
                }
            }
            _ => {}
        }
    }

    /// Text selection by dragging over a pane (its program doesn't want the
    /// mouse): a click starts it, dragging extends it (clamped to the pane)
    /// and releasing copies it. Returns true if the event was used up.
    fn on_selection_mouse(&mut self, mouse: MouseEvent, under: Option<((Pane, Rect), SessionKey)>) -> bool {
        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(_) => {
                self.selection = None;
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && matches!(self.mode, Mode::Normal)
                    && !mouse.modifiers.contains(KeyModifiers::SHIFT)
                    && let Some(((_, area), key)) = under
                    && self.sessions.contains_key(&key)
                    && !self.history_sessions.contains(&key)
                {
                    let cell = (pos.y - area.y, pos.x - area.x);
                    self.selection = Some(MouseSelection { key, anchor: cell, head: cell, done: false });
                }
                // The click also focuses the pane.
                false
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(selection) = self.selection.as_mut().filter(|s| !s.done) else { return false };
                let Some((_, area)) = self.pane_areas.iter().find(|((c, r), _)| {
                    self.columns.get(*c).and_then(|col| col.get(*r)) == Some(&selection.key)
                }) else {
                    return true;
                };
                let row = pos.y.clamp(area.y, area.bottom().saturating_sub(1)) - area.y;
                let col = pos.x.clamp(area.x, area.right().saturating_sub(1)) - area.x;
                selection.head = (row, col);
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(selection) = self.selection.as_mut().filter(|s| !s.done) else { return false };
                if selection.anchor == selection.head {
                    // A plain click: nothing selected.
                    self.selection = None;
                } else {
                    selection.done = true;
                    let (start, end) = selection.bounds();
                    self.request = Some(Request::CopySelection(selection.key, start, end));
                }
                true
            }
            _ => false,
        }
    }

    /// A click selects the row; a double click on the same row connects (or,
    /// in the Tags tab, filters by the tag).
    fn click_row(&mut self, row: usize) {
        let rows = match self.tab {
            ListTab::Connections => self.entries.len(),
            ListTab::Tags => self.tags.len(),
            ListTab::Layouts => self.layouts.len(),
        };
        if row >= rows {
            return;
        }
        self.mode = Mode::Normal;
        self.focus = Focus::List;
        match self.tab {
            ListTab::Connections => self.select(row),
            ListTab::Tags | ListTab::Layouts => self.select_side(row),
        }
        let double = self
            .last_click
            .is_some_and(|(r, at)| r == row && at.elapsed() < DOUBLE_CLICK);
        if double {
            self.last_click = None;
            match self.tab {
                ListTab::Connections => self.connect_selected(),
                ListTab::Tags => self.apply_tag(),
                ListTab::Layouts => self.open_layout(),
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
        assert_eq!(app.columns, [[SessionKey::ssh(2)], [SessionKey::sftp(2)]]);
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
    fn alt_n_also_cycles_through_sftp_sessions() {
        let mut app = app_with_sessions();
        app.sessions.insert(SessionKey::sftp(2), SessionState::Alive);
        app.on_key(ch(' '));
        app.on_key(alt('n'));
        assert_eq!(app.columns, [[SessionKey::sftp(2)]]);
        app.on_key(alt('n'));
        assert_eq!(app.columns, [[SessionKey::ssh(3)]]);
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
        app.pane_areas = vec![((0, 0), Rect::new(30, 1, 40, 20)), ((1, 0), Rect::new(72, 1, 40, 20))];
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
    fn dragging_over_a_pane_selects_and_copies() {
        let mut app = app_with_two_columns();
        app.pane_areas = vec![((0, 0), Rect::new(30, 1, 40, 20)), ((1, 0), Rect::new(72, 1, 40, 20))];
        let key = app.columns[0][0];
        app.sessions.insert(key, SessionState::Alive);
        let at = |kind, column, row| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
        let left = MouseButton::Left;
        // A plain click focuses the pane and selects nothing.
        app.on_mouse(at(MouseEventKind::Down(left), 35, 5));
        app.on_mouse(at(MouseEventKind::Up(left), 35, 5));
        assert_eq!((app.focus, app.selection, app.request.take()), (Focus::Terminal, None, None));
        // Dragging backwards and out of the pane: clamped to its first column.
        app.on_mouse(at(MouseEventKind::Down(left), 35, 5));
        app.on_mouse(at(MouseEventKind::Drag(left), 10, 3));
        let selection = app.selection.unwrap();
        assert_eq!(selection.bounds(), ((2, 0), (4, 5)));
        assert_eq!(selection.columns(2, 40), Some((0, 40)));
        assert_eq!(selection.columns(4, 40), Some((0, 6)));
        assert_eq!(selection.columns(5, 40), None);
        app.on_mouse(at(MouseEventKind::Up(left), 10, 3));
        assert_eq!(app.request.take(), Some(Request::CopySelection(key, (2, 0), (4, 5))));
        assert!(app.selection.is_some_and(|s| s.done));
        // The next key or click clears it.
        app.on_key(alt('l'));
        assert_eq!(app.selection, None);
        // Not while the program wants the mouse, nor with Shift.
        app.mouse_sessions.insert(key);
        app.on_mouse(at(MouseEventKind::Down(left), 35, 5));
        app.mouse_sessions.clear();
        let shift = MouseEvent { modifiers: KeyModifiers::SHIFT, ..at(MouseEventKind::Down(left), 35, 5) };
        app.on_mouse(shift);
        assert_eq!(app.selection, None);
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
    fn alt_v_on_a_tag_opens_its_connections_in_panes() {
        let mut app = app_with_two_columns();
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(ch(']'));
        app.on_key(ch('j'));
        app.on_key(ch('j'));
        app.on_key(alt('v'));
        // The panes are replaced; db1's session is alive, so only web1 opens.
        assert_eq!(columns(&app), ["web1", "db1"]);
        assert_eq!((app.focus, app.active_column, app.active_row), (Focus::Terminal, 0, 0));
        assert!(matches!(&app.request, Some(Request::OpenSessions(h)) if h.len() == 1 && h[0].0.id == 1));
        assert!(app.status.as_ref().is_some_and(|s| !s.error));
    }

    #[test]
    fn a_tag_is_stacked_or_cut_to_what_fits() {
        let open = |area| {
            let mut app = app();
            app.columns_area = area;
            app.on_key(ch(']'));
            app.on_key(ch('j'));
            app.on_key(ch('j'));
            app.on_key(alt('v'));
            app
        };
        // One column of two panes.
        let app = open(Rect::new(0, 0, 60, 12));
        assert_eq!(columns(&app), ["web1/db1"]);
        assert!(matches!(&app.request, Some(Request::OpenSessions(h)) if h.len() == 2));
        // Room for a single pane: the rest isn't opened.
        let app = open(Rect::new(0, 0, 60, 8));
        assert_eq!(columns(&app), ["web1"]);
        assert!(matches!(&app.request, Some(Request::OpenSessions(h)) if h.len() == 1));
        assert!(app.status.as_ref().is_some_and(|s| s.error));
    }

    fn layout() -> Layout {
        // Connection 99 no longer exists.
        Layout { name: "ops".into(), columns: vec![vec![(1, false), (2, true)], vec![(99, false)], vec![(3, false)]] }
    }

    #[test]
    fn layouts_tab_saves_the_panes_under_a_name() {
        let mut app = app();
        app.on_key(ch('['));
        assert_eq!(app.tab, ListTab::Layouts);
        // Nothing to save without panes.
        app.on_key(ch('a'));
        assert!(matches!(app.mode, Mode::Normal) && app.status.as_ref().is_some_and(|s| s.error));

        let mut app = app_with_two_columns();
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(ch('['));
        app.on_key(ch('a'));
        // An empty name isn't accepted; Esc cancels.
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::LayoutName { name, rename: None } if name.is_empty()));
        for c in " opx".chars() {
            app.on_key(ch(c));
        }
        app.on_key(key(KeyCode::Backspace));
        app.on_paste("s\n");
        app.on_key(key(KeyCode::Enter));
        let saved = Layout { name: "ops".into(), columns: vec![vec![(2, false)], vec![(3, false)]] };
        assert_eq!(app.request.take(), Some(Request::SaveLayout(saved)));
        assert!(matches!(app.mode, Mode::Normal));
        app.on_key(ch('a'));
        app.on_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal) && app.request.is_none());
    }

    #[test]
    fn space_on_a_layout_restores_its_panes() {
        let mut app = app_with_sessions();
        app.set_layouts(vec![layout()], None);
        app.on_key(ch('['));
        app.on_key(ch(' '));
        assert_eq!(columns(&app), ["web1/db1", "dev"]);
        assert_eq!(app.columns[0][1], SessionKey::sftp(2));
        assert_eq!((app.focus, app.active_column, app.active_row), (Focus::Terminal, 0, 0));
        // db1's live session is ssh, not the sftp one of the layout.
        let Some(Request::OpenSessions(missing)) = app.request.take() else { panic!("no sessions requested") };
        let missing: Vec<(i64, SessionKind)> = missing.iter().map(|(h, k)| (h.id, *k)).collect();
        assert_eq!(missing, [(1, SessionKind::Ssh), (2, SessionKind::Sftp), (3, SessionKind::Ssh)]);

        // Too small for the saved shape: rearranged, or cut.
        let mut app = app_with_sessions();
        app.set_layouts(vec![layout()], None);
        app.columns_area = Rect::new(0, 0, 60, 18);
        app.on_key(ch('['));
        app.on_key(ch(' '));
        assert_eq!(columns(&app), ["web1/db1/dev"]);
        assert!(app.status.as_ref().is_some_and(|s| !s.error));
        app.on_key(alt('h'));
        app.columns_area = Rect::new(0, 0, 60, 12);
        app.on_key(ch(' '));
        assert_eq!(columns(&app), ["web1/db1"]);
        assert!(app.status.as_ref().is_some_and(|s| s.error));
    }

    #[test]
    fn r_renames_a_layout() {
        let mut app = app();
        app.set_layouts(vec![layout()], None);
        app.on_key(ch('['));
        app.on_key(ch('r'));
        assert!(matches!(&app.mode, Mode::LayoutName { name, rename: Some(r) } if name == "ops" && r == "ops"));
        // The same name changes nothing.
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal) && app.request.is_none());
        app.on_key(ch('r'));
        for _ in 0..3 {
            app.on_key(key(KeyCode::Backspace));
        }
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::LayoutName { .. }));
        app.on_paste("prod ");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.request.take(), Some(Request::RenameLayout { from: "ops".into(), to: "prod".into() }));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn dd_deletes_a_layout_after_confirming() {
        let mut app = app();
        let empty = Layout { name: "old".into(), columns: Vec::new() };
        app.set_layouts(vec![empty, layout()], Some("ops"));
        app.on_key(ch('['));
        assert_eq!(app.selected_layout().unwrap().name, "ops");
        app.on_key(ch('d'));
        app.on_key(ch('d'));
        assert!(matches!(&app.mode, Mode::ConfirmDeleteLayout { name } if name == "ops"));
        app.on_key(ch('n'));
        assert_eq!(app.request, None);
        app.on_key(ch('d'));
        app.on_key(ch('d'));
        app.on_key(ch('y'));
        assert_eq!(app.request.take(), Some(Request::DeleteLayout("ops".into())));
        // A layout whose connections were all deleted opens nothing.
        app.on_key(ch('k'));
        app.on_key(ch(' '));
        assert!(app.columns.is_empty() && app.status.as_ref().is_some_and(|s| s.error));
    }

    #[test]
    fn tags_tab_ignores_connection_actions() {
        let mut app = app();
        app.on_key(ch(']'));
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

    fn names(aliases: &[&str]) -> Vec<String> {
        aliases.iter().map(|a| a.to_string()).collect()
    }

    /// One string per column; stacked panes are joined with "/" (top first).
    fn columns(app: &App) -> Vec<String> {
        app.columns
            .iter()
            .map(|panes| {
                let aliases: Vec<&str> = panes.iter().map(|k| app.host(k.host).unwrap().data.alias.as_str()).collect();
                aliases.join("/")
            })
            .collect()
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
    fn the_list_shrinks_to_make_room_for_columns() {
        let mut app = app_with_two_columns();
        // Two columns of 60 now, but the list can shrink to leave 160.
        app.columns_area = Rect::new(80, 0, 120, 30);
        assert_eq!(app.max_columns(), 3);
        app.columns_room = 160;
        assert_eq!(app.max_columns(), 4);
    }

    #[test]
    fn portrait_stacks_the_panes_and_limits_them_by_height() {
        let mut app = app_with_two_columns();
        // Narrow and 12 lines high: room for two panes of 6 lines.
        (app.portrait, app.stacked) = (true, true);
        app.columns_area = Rect::new(0, 0, 80, 12);
        // Alt-j/k go through every pane in order; Alt-h goes back to the list.
        app.on_key(alt('k'));
        assert_eq!((app.active_column, app.active_row), (0, 0));
        app.on_key(alt('j'));
        app.on_key(alt('j'));
        assert_eq!((app.active_column, app.active_row), (1, 0));
        app.on_key(alt('h'));
        assert_eq!(app.focus, Focus::List);
        // A third pane doesn't fit, whatever its width.
        let web1 = app.entries.iter().position(|e| app.hosts[e.index].id == 1).unwrap();
        app.select(web1);
        app.on_key(alt('v'));
        assert_eq!(columns(&app), ["db1", "dev"]);
        assert!(app.status.as_ref().is_some_and(|s| s.error && s.text.contains("lines")));
        // Moving panes around is fine: there are as many.
        app.on_key(alt('l'));
        app.on_key(alt('J'));
        assert_eq!(columns(&app), ["dev/db1"]);
        // A layout that doesn't fit is cut, one pane per column.
        app.set_layouts(vec![layout()], None);
        app.on_key(alt('h'));
        app.on_key(ch('['));
        app.on_key(ch(' '));
        assert_eq!(columns(&app), ["web1", "db1"]);
    }

    #[test]
    fn space_shows_the_selection_in_the_active_column() {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        // db1's session is alive: shown without reopening it.
        assert_eq!((columns(&app), app.focus, &app.request), (names(&["db1"]), Focus::Terminal, &None));
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
    fn alt_a_broadcasts_input_to_the_visible_panes() {
        let mut app = app_with_two_columns();
        app.sessions.insert(SessionKey::ssh(3), SessionState::Alive);
        let (db1, dev) = (SessionKey::ssh(2), SessionKey::ssh(3));
        assert_eq!(app.input_targets(dev), [dev]);
        app.on_key(alt('a'));
        assert!(app.broadcast && app.request.is_none());
        // The key still targets the active pane; the loop fans it out.
        app.on_key(ch('x'));
        assert_eq!(app.request.take(), Some(Request::Input(dev, ch('x'))));
        assert_eq!(app.input_targets(dev), [dev, db1]);
        // Ended sessions, history mode and other kinds are left out.
        app.history_sessions.insert(db1);
        assert_eq!(app.input_targets(dev), [dev]);
        app.history_sessions.clear();
        assert_eq!(app.input_targets(SessionKey::sftp(3)), [SessionKey::sftp(3)]);
        app.sessions.insert(db1, SessionState::Ended);
        assert_eq!(app.input_targets(dev), [dev]);
        app.sessions.insert(db1, SessionState::Alive);
        // Full screen shows a single pane.
        app.on_key(alt('f'));
        assert_eq!(app.input_targets(dev), [dev]);
        app.on_key(alt('f'));
        app.on_key(alt('a'));
        assert!(!app.broadcast);
        assert_eq!(app.input_targets(dev), [dev]);
    }

    #[test]
    fn broadcast_needs_panes_and_stops_without_them() {
        let mut app = app_with_sessions();
        app.on_key(alt('a'));
        assert!(!app.broadcast && app.status.as_ref().is_some_and(|s| s.error));
        app.on_key(ch(' '));
        app.on_key(alt('a'));
        assert!(app.broadcast);
        app.on_key(alt('w'));
        assert!(app.columns.is_empty() && !app.broadcast);
    }

    #[test]
    fn alt_l_needs_a_column() {
        let mut app = app_with_sessions();
        app.on_key(alt('l'));
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn alt_n_p_switch_the_session_of_the_pane() {
        let mut app = app_with_sessions();
        app.on_key(ch(' '));
        app.on_key(alt('n'));
        assert_eq!(columns(&app), ["dev"]);
        app.on_key(alt('p'));
        assert_eq!(columns(&app), ["db1"]);
        // Sessions visible in another pane are skipped.
        let mut app = app_with_two_columns();
        app.on_key(alt('n'));
        assert_eq!(columns(&app), ["db1", "dev"]);
        // In the list, Alt-j/k move the selection between sessions.
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(alt('j'));
        assert_eq!(app.selected_host().unwrap().id, 3);
    }

    fn active(app: &App) -> (usize, usize) {
        (app.active_column, app.active_row)
    }

    #[test]
    fn alt_shift_j_stacks_the_pane_below_its_right_neighbour() {
        let mut app = app_with_two_columns();
        app.on_key(alt('h'));
        app.on_key(alt('J'));
        assert_eq!(columns(&app), ["dev/db1"]);
        assert_eq!(active(&app), (0, 1));
        // The last column goes below the one on its left.
        let mut app = app_with_two_columns();
        app.on_key(alt('J'));
        assert_eq!((columns(&app), active(&app)), (names(&["db1/dev"]), (0, 1)));
        // A single column has nowhere to go.
        app.on_key(alt('J'));
        assert_eq!(columns(&app), ["db1/dev"]);
    }

    #[test]
    fn alt_shift_k_takes_the_pane_out_and_alt_j_k_move_between_panes() {
        let mut app = app_with_two_columns();
        app.on_key(alt('J'));
        app.on_key(alt('k'));
        assert_eq!(active(&app), (0, 0));
        assert_eq!(app.selected_host().unwrap().data.alias, "db1");
        app.on_key(alt('j'));
        assert_eq!(active(&app), (0, 1));
        app.on_key(alt('j'));
        assert_eq!(active(&app), (0, 1));
        app.on_key(alt('K'));
        assert_eq!((columns(&app), active(&app)), (names(&["db1", "dev"]), (1, 0)));
        // Already alone in its column: nothing to do.
        app.on_key(alt('K'));
        assert_eq!(columns(&app), ["db1", "dev"]);
    }

    #[test]
    fn alt_minus_opens_a_pane_below_the_last_column() {
        let mut app = app_with_two_columns();
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(ch('g'));
        app.on_key(ch('g'));
        app.on_key(alt('-'));
        assert_eq!((columns(&app), active(&app)), (names(&["db1", "dev/web1"]), (1, 1)));
        assert!(matches!(&app.request, Some(Request::OpenSession(h, SessionKind::Ssh)) if h.id == 1));
        // With no columns it opens the first one.
        let mut app = self::app();
        app.on_key(alt('-'));
        assert_eq!(columns(&app), ["web1"]);
    }

    #[test]
    fn closing_a_stacked_pane_gives_its_space_to_the_others() {
        let mut app = app_with_two_columns();
        app.on_key(alt('J'));
        app.on_key(alt('w'));
        assert_eq!((columns(&app), active(&app), app.focus), (names(&["db1"]), (0, 0), Focus::Terminal));
    }

    #[test]
    fn panes_need_a_minimum_height() {
        let mut app = app_with_two_columns();
        app.columns_area = Rect::new(30, 0, 100, 12);
        app.on_key(alt('J'));
        assert_eq!(columns(&app), ["db1/dev"]);
        app.on_key(alt('h'));
        app.on_key(ch('g'));
        app.on_key(ch('g'));
        app.on_key(alt('-'));
        assert_eq!(columns(&app), ["db1/dev"]);
        assert!(app.status.as_ref().unwrap().error);
    }

    #[test]
    fn moving_between_columns_keeps_the_row() {
        let mut app = app_with_two_columns();
        app.on_key(alt('h'));
        app.on_key(alt('h'));
        app.on_key(ch('g'));
        app.on_key(ch('g'));
        app.on_key(alt('-'));
        // Columns: db1 | dev/web1, focus on web1 (1, 1).
        app.on_key(alt('h'));
        assert_eq!(active(&app), (0, 0));
        app.on_key(alt('l'));
        assert_eq!(active(&app), (1, 0));
    }

    #[test]
    fn alt_w_closes_columns_but_keeps_sessions() {
        let mut app = app_with_two_columns();
        app.on_key(alt('w'));
        assert_eq!((columns(&app), app.active_column, app.focus), (names(&["db1"]), 0, Focus::Terminal));
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
        assert_eq!((columns(&app), app.active_column), (names(&["dev", "db1"]), 0));
        app.on_key(alt('L'));
        assert_eq!((columns(&app), app.active_column), (names(&["db1", "dev"]), 1));
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
        app.pane_areas = vec![((0, 0), Rect::new(30, 1, 40, 20)), ((1, 0), Rect::new(72, 1, 40, 20))];
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

    fn themes() -> Vec<(Theme, bool)> {
        let builtin = |name: &str| {
            let (_, source) = crate::theme::BUILTIN.iter().find(|(n, _)| *n == name).unwrap();
            (Theme::parse(name, source, crate::theme::Depth::TrueColor).unwrap(), false)
        };
        vec![(Theme::terminal(), false), builtin("gruvbox-dark"), builtin("monokai")]
    }

    #[test]
    fn theme_picker_previews_and_saves() {
        let mut app = app();
        app.on_key(ch('T'));
        assert_eq!(app.request.take(), Some(Request::Themes));
        app.open_themes(themes());
        // Moving shows the theme right away; Enter saves it.
        app.on_key(ch('j'));
        assert_eq!(app.theme.name, "gruvbox-dark");
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.theme.name, "monokai");
        assert_eq!(app.request.take(), Some(Request::SaveTheme("monokai".into())));
        assert!(matches!(app.mode, Mode::Normal));
        // It opens on the theme in use; k from the top wraps around.
        app.open_themes(themes());
        assert!(matches!(&app.mode, Mode::Themes(p) if p.selected == 2));
        app.on_key(ch('j'));
        assert_eq!(app.theme.name, "terminal");
    }

    #[test]
    fn theme_picker_esc_restores_the_previous_theme() {
        let mut app = app();
        app.open_themes(themes());
        app.on_key(ch('k'));
        assert_eq!(app.theme.name, "monokai");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.theme.name, "terminal");
        assert_eq!(app.request, None);
        assert!(matches!(app.mode, Mode::Normal));
        // Ctrl-c cancels too (it doesn't quit from the picker).
        app.open_themes(themes());
        app.on_key(ch('j'));
        app.on_key(ctrl('c'));
        assert_eq!(app.theme.name, "terminal");
        assert_eq!(app.outcome, None);
    }

    fn help(app: &App) -> &HelpState {
        match &app.mode {
            Mode::Help(help) => help,
            _ => panic!("the help is not open"),
        }
    }

    /// Opens the help as if it were drawn `height` lines high.
    fn open_help(app: &mut App, height: usize) {
        app.on_key(ch('?'));
        if let Mode::Help(help) = &mut app.mode {
            help.height = height;
        }
    }

    fn type_keys(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(ch(c));
        }
    }

    #[test]
    fn help_search_highlights_and_jumps() {
        let mut app = app();
        open_help(&mut app, 10);
        type_keys(&mut app, "/broadcast");
        let h = help(&app);
        assert!(h.typing);
        assert!(!h.matches.is_empty());
        let first = h.current_line().unwrap();
        assert!(HELP[first].1.contains("broadcast"), "{:?}", HELP[first]);
        // The matched chars of the description are highlighted.
        let (_, desc) = h.highlights(first);
        assert_eq!(desc.len(), "broadcast".len());
        app.on_key(key(KeyCode::Enter));
        assert!(!help(&app).typing);
        // `n` wraps around, `N` goes back.
        let count = help(&app).matches.len();
        for _ in 0..count {
            app.on_key(ch('n'));
        }
        assert_eq!(help(&app).current_line(), Some(first));
        app.on_key(ch('N'));
        assert_eq!(help(&app).current, Some(count - 1));
    }

    #[test]
    fn help_search_scrolls_to_the_current_match() {
        let mut app = app();
        open_help(&mut app, 5);
        type_keys(&mut app, "/wheel");
        app.on_key(key(KeyCode::Enter));
        let h = help(&app);
        let line = h.current_line().unwrap();
        assert!(line >= 5, "the match should be below the first page");
        assert!((h.scroll..h.scroll + 5).contains(&line));
        assert!(h.scroll <= HELP.len() - 5);
    }

    #[test]
    fn help_search_without_matches() {
        let mut app = app();
        open_help(&mut app, 10);
        type_keys(&mut app, "/zzzzqqq");
        let h = help(&app);
        assert!(h.matches.is_empty());
        assert_eq!(h.current, None);
        app.on_key(key(KeyCode::Enter));
        app.on_key(ch('n'));
        assert_eq!(help(&app).current, None);
    }

    #[test]
    fn help_scrolls_and_esc_clears_then_closes() {
        let mut app = app();
        open_help(&mut app, 10);
        app.on_key(ch('j'));
        app.on_key(ch('j'));
        assert_eq!(help(&app).scroll, 2);
        app.on_key(ch('k'));
        assert_eq!(help(&app).scroll, 1);
        app.on_key(ch('G'));
        assert_eq!(help(&app).scroll, HELP.len() - 10);
        app.on_key(ch('j'));
        assert_eq!(help(&app).scroll, HELP.len() - 10);
        app.on_key(ch('g'));
        app.on_key(ch('g'));
        assert_eq!(help(&app).scroll, 0);
        app.on_key(ctrl('d'));
        assert_eq!(help(&app).scroll, 5);
        // Other keys don't close it any more.
        app.on_key(ch('x'));
        assert!(matches!(app.mode, Mode::Help(_)));
        // Esc in the prompt cancels the search; then it clears a kept one,
        // then closes.
        type_keys(&mut app, "/tag");
        app.on_key(key(KeyCode::Esc));
        assert!(help(&app).query.is_empty() && !help(&app).typing);
        type_keys(&mut app, "/tag");
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Esc));
        assert!(help(&app).query.is_empty());
        assert!(help(&app).matches.is_empty());
        app.on_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
        // q and ? close it too.
        for c in ['q', '?'] {
            open_help(&mut app, 10);
            app.on_key(ch(c));
            assert!(matches!(app.mode, Mode::Normal));
        }
    }
}
