//! TUI rendering.
//!
//! Layout (lazygit style): search, connection list and details on the left;
//! the embedded ssh session of the selected connection on the right.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, Wrap};
use tui_term::widget::{Cursor, PseudoTerminal};

use super::app::{App, Focus, ListTab, Mode, Pane, SessionKey, SessionKind, SessionState};
use super::session::Session;
use crate::cli::{format_date, relative_time};
use crate::connect::shell_quote;
use crate::db;
use crate::model::{Host, HostData};

// Only the 16 ANSI colors (plus the terminal's default foreground and
// background), so the UI follows the terminal theme (e.g. Omarchy themes,
// where color4 / blue is the primary accent).
pub const ACCENT: Color = Color::Blue;
const MATCH: Color = Color::Yellow;
const TAG: Color = Color::Magenta;
const CONNECTED: Color = Color::Green;

pub fn draw(frame: &mut Frame, app: &mut App, sessions: &HashMap<SessionKey, Session>) {
    let [main, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    app.pane_areas.clear();
    if app.zoomed && !app.columns.is_empty() {
        // Only the active pane, on the whole screen.
        app.rows_area = Rect::default();
        app.search_area = Rect::default();
        app.detail_area = Rect::default();
        app.columns_area = main;
        draw_pane(frame, app, sessions, (app.active_column, app.active_row), main);
    } else {
        let left_width = (main.width * 24 / 100).clamp(24, 40).min(main.width);
        let [left, right] =
            Layout::horizontal([Constraint::Length(left_width), Constraint::Fill(1)]).areas(main);
        let [search, list, detail] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Percentage(55),
            Constraint::Fill(1),
        ])
        .areas(left);
        draw_search(frame, app, search);
        draw_list(frame, app, list);
        draw_detail(frame, app, detail);
        draw_columns(frame, app, sessions, right);
    }
    draw_footer(frame, app, footer);
    match &mut app.mode {
        Mode::Help => draw_help(frame),
        Mode::Form(form) => form.render(frame),
        Mode::ConfirmDelete { alias } => draw_confirm(
            frame,
            vec!["Delete connection ".into(), alias.clone().bold(), "?".into()],
            "delete",
        ),
        Mode::ConfirmClose { alias, .. } => draw_confirm(
            frame,
            vec!["Close the session of ".into(), alias.clone().bold(), "?".into()],
            "close",
        ),
        Mode::ConfirmQuit { sessions } => draw_confirm(
            frame,
            vec![format!("Close {sessions} open session(s) and quit?").into()],
            "quit",
        ),
        _ => {}
    }
}

pub fn block(title: &str, focused: bool) -> Block<'_> {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(" {title} "));
    if focused {
        block.border_style(Style::new().fg(ACCENT)).title_style(Style::new().bold())
    } else {
        block.border_style(Style::new().dim())
    }
}

fn pane_focused(app: &App, focus: Focus) -> bool {
    matches!(app.mode, Mode::Normal) && app.focus == focus
}

fn draw_search(frame: &mut Frame, app: &mut App, area: Rect) {
    app.search_area = area;
    let searching = matches!(app.mode, Mode::Search);
    let content = if app.query.is_empty() && !searching {
        Line::from("/ search · #tag".dim())
    } else {
        Line::from(app.query.as_str())
    };
    let block = block("Search", searching);
    let inner = block.inner(area);
    frame.render_widget(Paragraph::new(content).block(block), area);
    if searching {
        let x = inner.x + (app.query.chars().count() as u16).min(inner.width.saturating_sub(1));
        frame.set_cursor_position((x, inner.y));
    }
}

/// Text with the chars at `positions` highlighted.
fn highlighted<'a>(text: &'a str, positions: &[usize]) -> Vec<Span<'a>> {
    if positions.is_empty() {
        return vec![Span::raw(text)];
    }
    let hl = Style::new().fg(MATCH).bold();
    text.chars()
        .enumerate()
        .map(|(i, c)| {
            let span = Span::raw(c.to_string());
            if positions.contains(&i) { span.style(hl) } else { span }
        })
        .collect()
}

fn session_dot(state: Option<SessionState>) -> Span<'static> {
    match state {
        Some(SessionState::Alive) => "●".fg(CONNECTED),
        Some(SessionState::Ended) => "○".dim(),
        None => " ".into(),
    }
}

/// Bordered block for the list pane with its tabs as the title; records where
/// each tab was drawn so it can be clicked.
fn list_block(app: &mut App, area: Rect) -> Block<'static> {
    let focused = pane_focused(app, Focus::List);
    let tabs = [
        (ListTab::Connections, format!(" Connections {}/{} ", app.entries.len(), app.hosts.len())),
        (ListTab::Tags, format!(" Tags {} ", app.tags.len())),
    ];
    let mut spans = Vec::new();
    let mut x = area.x + 1;
    app.tab_areas.clear();
    for (i, (tab, text)) in tabs.into_iter().enumerate() {
        if i > 0 {
            spans.push("·".dim());
            x += 1;
        }
        let width = text.chars().count() as u16;
        app.tab_areas.push((tab, Rect::new(x, area.y, width, 1)));
        x += width;
        let style = match (tab == app.tab, focused) {
            (true, true) => Style::new().fg(ACCENT).bold(),
            (true, false) => Style::new().bold(),
            (false, _) => Style::new().dim(),
        };
        spans.push(Span::styled(text, style));
    }
    let mut block = Block::bordered().border_type(BorderType::Rounded).title(Line::from(spans));
    if app.tab == ListTab::Connections {
        // On the bottom border: the top one is for the tabs.
        block = block.title_bottom(Line::from(format!(" {} ", app.sort.label()).dim()).right_aligned());
    }
    if focused { block.border_style(Style::new().fg(ACCENT)) } else { block.border_style(Style::new().dim()) }
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = list_block(app, area);
    app.rows_area = block.inner(area);
    if app.tab == ListTab::Tags {
        draw_tags(frame, app, block, area);
        return;
    }

    if app.entries.is_empty() {
        let msg = if app.hosts.is_empty() { "No connections yet.\nPress 'a' to add one." } else { "No results." };
        let p = Paragraph::new(msg).dim().centered().wrap(Wrap { trim: false });
        let [middle] = Layout::vertical([Constraint::Length(2)]).flex(Flex::Center).areas(app.rows_area);
        frame.render_widget(block, area);
        frame.render_widget(p, middle);
        return;
    }

    // Computed first, so the rows below only borrow fields of `app`, not `app`.
    let states: Vec<Option<SessionState>> =
        app.entries.iter().map(|e| app.host_session(app.hosts[e.index].id)).collect();
    let rows = app.entries.iter().zip(states).map(|(e, state)| {
        let host = &app.hosts[e.index];
        let d = &host.data;
        let mut spans = highlighted(&d.alias, &e.alias_hl);
        if let Some(name) = &d.name {
            spans.push(Span::raw("  "));
            spans.extend(highlighted(name, &e.name_hl).into_iter().map(|s| s.dim()));
        }
        if !d.tags.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(tags_text(d).fg(TAG).dim());
        }
        Row::new([Cell::from(session_dot(state)), Cell::from(Line::from(spans))])
    });
    let table = Table::new(rows, [Constraint::Length(1), Constraint::Fill(1)])
        .block(block)
        .column_spacing(1)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_spacing(HighlightSpacing::Never);
    frame.render_stateful_widget(table, area, &mut app.table);
}

/// The Tags tab: every tag with its number of connections.
fn draw_tags(frame: &mut Frame, app: &mut App, block: Block<'static>, area: Rect) {
    if app.tags.is_empty() {
        let p = Paragraph::new("No tags yet.\nAdd them with 't'.").dim().centered();
        let [middle] = Layout::vertical([Constraint::Length(2)]).flex(Flex::Center).areas(app.rows_area);
        frame.render_widget(block, area);
        frame.render_widget(p, middle);
        return;
    }
    let rows = app.tags.iter().map(|t| {
        Row::new([
            Cell::from(format!("#{}", t.name).fg(TAG)),
            Cell::from(Line::from(t.count.to_string()).right_aligned().dim()),
        ])
    });
    let table = Table::new(rows, [Constraint::Fill(1), Constraint::Length(5)])
        .block(block)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_spacing(HighlightSpacing::Never);
    frame.render_stateful_widget(table, area, &mut app.tag_table);
}

fn tags_text(d: &HostData) -> String {
    d.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")
}

fn tags_line(host: &Host) -> Line<'static> {
    Line::from(host.data.tags.iter().map(|t| format!("#{t} ").fg(TAG)).collect::<Vec<_>>())
}

fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    app.detail_area = area;
    if app.tab == ListTab::Tags {
        draw_tag_preview(frame, app, area);
        return;
    }
    let block = block("Details", pane_focused(app, Focus::Detail));
    let Some(host) = app.selected_host() else {
        frame.render_widget(block, area);
        return;
    };
    let d = &host.data;
    let now = db::now();
    let field = |label: &'static str, value: String| {
        Line::from(vec![format!("{label:<9}").fg(ACCENT), value.into()])
    };

    let mut lines = vec![Line::from(d.name.clone().unwrap_or_else(|| d.alias.clone()).bold())];
    if let Some(desc) = &d.description {
        lines.push(Line::from(desc.clone().italic()));
    }
    lines.push(field("Target", d.target()));
    if let Some(identity) = &d.identity_file {
        lines.push(field("Identity", identity.clone()));
    }
    if let Some(jump) = &d.proxy_jump {
        lines.push(field("Jump", jump.clone()));
    }
    for opt in &d.extra_options {
        lines.push(field("Option", format!("{} {}", opt.key, opt.value)));
    }
    if !d.tags.is_empty() {
        lines.push(tags_line(host));
    }
    let usage = match host.last_used {
        Some(t) => {
            let times = if host.use_count == 1 { "time" } else { "times" };
            format!("{} {times} · {}", host.use_count, relative_time(now - t))
        }
        None => "never".into(),
    };
    lines.push(field("Used", usage));
    lines.push(field("Created", format_date(host.created_at)));
    if let Some(notes) = &d.notes {
        lines.push(Line::default());
        lines.extend(Text::from(notes.as_str()).lines);
    }

    let p = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    // Clamp the scroll so the pane never ends up empty.
    let max_scroll = (p.line_count(area.width) as u16).saturating_sub(area.height);
    let scroll = app.detail_scroll.min(max_scroll);
    frame.render_widget(p.scroll((scroll, 0)), area);
    app.detail_scroll = scroll;
}

/// Details pane in the Tags tab: the connections of the highlighted tag.
fn draw_tag_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(tag) = app.selected_tag() else {
        frame.render_widget(block("Details", false), area);
        return;
    };
    let noun = if tag.count == 1 { "connection" } else { "connections" };
    let title = format!("#{} · {} {noun}", tag.name, tag.count);
    let lines: Vec<Line> = app
        .hosts
        .iter()
        .filter(|h| h.data.tags.contains(&tag.name))
        .map(|h| {
            Line::from(vec![
                session_dot(app.host_session(h.id)),
                " ".into(),
                h.data.alias.clone().bold(),
                "  ".into(),
                h.data.target().dim(),
            ])
        })
        .collect();
    let block = block(&title, pane_focused(app, Focus::Detail));
    let p = Paragraph::new(lines).block(block);
    let max_scroll = (p.line_count(area.width) as u16).saturating_sub(area.height);
    let scroll = app.detail_scroll.min(max_scroll);
    frame.render_widget(p.scroll((scroll, 0)), area);
    app.detail_scroll = scroll;
}

/// Right side: the terminal columns, or an overview of the selected
/// connection when no column is open.
fn draw_columns(frame: &mut Frame, app: &mut App, sessions: &HashMap<SessionKey, Session>, area: Rect) {
    app.columns_area = area;
    if app.columns.is_empty() {
        draw_overview(frame, app, area);
        return;
    }
    let columns = Layout::horizontal(vec![Constraint::Fill(1); app.columns.len()]).split(area);
    for (c, column) in columns.iter().enumerate() {
        let panes = Layout::vertical(vec![Constraint::Fill(1); app.columns[c].len()]).split(*column);
        for (r, pane) in panes.iter().enumerate() {
            draw_pane(frame, app, sessions, (c, r), *pane);
        }
    }
}

/// Right side when no column is open: the logo and the selected connection's
/// actions and latest connections.
fn draw_overview(frame: &mut Frame, app: &App, area: Rect) {
    let host = app.selected_host();
    let title = match host {
        None => "sshh".to_string(),
        Some(host) => match app.host_session(host.id) {
            Some(SessionState::Alive) => format!("{} — session running · Space shows it", host.data.alias),
            Some(SessionState::Ended) => format!("{} — session ended · Space reconnects", host.data.alias),
            None => format!("{} — not connected", host.data.alias),
        },
    };
    let block = block(&title, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (body, history) = match host {
        Some(host) => (overview_lines(host), history_lines(app)),
        None if app.hosts.is_empty() => (
            vec![
                Line::from("No saved connections.".bold()),
                Line::default(),
                Line::from("Press 'a' to add one, or import your ~/.ssh/config"),
                Line::from("with `sshh import-ssh-config`."),
            ],
            Vec::new(),
        ),
        None => (vec![Line::from("No connection selected.".dim())], Vec::new()),
    };

    // When space is short the history goes first, then the logo.
    let logo = logo_lines();
    let height = usize::from(inner.height);
    let wide = inner.width >= 36;
    let fits = |lens: &[usize]| lens.iter().map(|n| n + 1).sum::<usize>() <= height;
    let (show_logo, show_history) = if wide && fits(&[logo.len(), body.len(), history.len()]) {
        (true, true)
    } else if wide && fits(&[logo.len(), body.len()]) {
        (true, false)
    } else {
        (false, true)
    };

    // The actions and the history are centred together so they line up.
    let mut info = body;
    if show_history && !history.is_empty() {
        info.push(Line::default());
        info.extend(history);
    }
    let mut lines = Vec::new();
    if show_logo {
        lines.extend(center_block(logo, inner.width));
        lines.push(Line::default());
    }
    lines.extend(center_block(info, inner.width));
    let top = inner.height.saturating_sub(lines.len() as u16) / 2;
    let area = Rect { y: inner.y + top, height: inner.height - top, ..inner };
    frame.render_widget(Paragraph::new(lines), area);
}

/// "SSHH" in the figlet "ANSI Shadow" font.
const WORDMARK: [&str; 6] = [
    "███████╗███████╗██╗  ██╗██╗  ██╗",
    "██╔════╝██╔════╝██║  ██║██║  ██║",
    "███████╗███████╗███████║███████║",
    "╚════██║╚════██║██╔══██║██╔══██║",
    "███████║███████║██║  ██║██║  ██║",
    "╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝",
];
const TAGLINE: &str = "ssh connection manager";

/// The logo: the wordmark in a cyan → blue → magenta gradient (taken from the
/// terminal theme) and the tagline.
fn logo_lines() -> Vec<Line<'static>> {
    let gradient = [
        Color::LightCyan,
        Color::Cyan,
        Color::LightBlue,
        Color::Blue,
        Color::LightMagenta,
        Color::Magenta,
    ];
    let mut lines: Vec<Line> = WORDMARK
        .iter()
        .zip(gradient)
        .map(|(row, color)| Line::from(row.fg(color)))
        .collect();
    let width = WORDMARK[0].chars().count();
    let pad = (width - TAGLINE.len()) / 2;
    lines.push(Line::default());
    lines.push(Line::from(format!("{}{TAGLINE}", " ".repeat(pad)).dim()));
    lines
}

/// Indents `lines` so that they are centred as a block in `width` columns
/// (centring each line on its own would distort ASCII art).
fn center_block(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let block_width = lines.iter().map(Line::width).max().unwrap_or(0);
    let pad = usize::from(width).saturating_sub(block_width) / 2;
    lines
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, Span::raw(" ".repeat(pad)));
            line
        })
        .collect()
}

/// One pane of the terminal columns: the session it shows.
fn draw_pane(
    frame: &mut Frame,
    app: &mut App,
    sessions: &HashMap<SessionKey, Session>,
    (column, row): Pane,
    area: Rect,
) {
    app.pane_areas.push(((column, row), Block::bordered().inner(area)));
    let focused =
        pane_focused(app, Focus::Terminal) && (column, row) == (app.active_column, app.active_row);
    let key = app.columns[column][row];
    let Some(host) = app.host(key.host) else { return };
    // "2" for a column with a single pane, "2.1", "2.2"… when they are stacked.
    let number = match app.columns[column].len() {
        1 => (column + 1).to_string(),
        _ => format!("{}.{}", column + 1, row + 1),
    };
    // ssh columns show the target; sftp ones say so.
    let what = match key.kind {
        SessionKind::Ssh => host.data.target(),
        SessionKind::Sftp => "sftp".to_string(),
    };

    let Some(session) = sessions.get(&key) else {
        let title = format!("{number} {} — {what} · no session · Enter connects", host.data.alias);
        let text = vec![Line::default(), Line::from(vec!["  ".into(), host.data.target().bold()])];
        frame.render_widget(Paragraph::new(text).block(block(&title, focused)), area);
        return;
    };

    let parser = session.parser();
    let screen = parser.screen();
    let copy = session.copy_mode();
    let mut title = match (copy, session.exit()) {
        (Some(copy), _) => {
            let mut title = format!("{number} {} — history {}/{}", host.data.alias, copy.cursor + 1, copy.total());
            if let Some(prompt) = &copy.prompt {
                title.push_str(&format!(" · /{prompt}█"));
            } else if let Some(message) = &copy.message {
                title.push_str(&format!(" · {message}"));
            }
            title
        }
        (None, None) => format!("{number} ● {} — {what}", host.data.alias),
        (None, Some(exit)) => format!("{number} {} — {what} · {exit} · Enter reconnects", host.data.alias),
    };
    if copy.is_none() && screen.scrollback() > 0 {
        title.push_str(&format!(" · scrolled back {}", screen.scrollback()));
    }
    if app.zoomed {
        title.push_str(" · full screen (Alt-f)");
    }
    let mut block = block(&title, focused);
    if session.exit().is_none() {
        let style = Style::new().fg(CONNECTED);
        block = block.title_style(if focused { style.bold() } else { style });
    }
    let show_cursor = focused
        && copy.is_none()
        && session.exit().is_none()
        && !screen.hide_cursor()
        && screen.scrollback() == 0;
    let terminal = PseudoTerminal::new(screen)
        .block(block)
        .cursor(Cursor::default().visibility(show_cursor));
    frame.render_widget(terminal, area);

    // History mode: highlight the cursor line or the selection.
    if let Some(copy) = copy {
        let inner = Block::bordered().inner(area);
        let top = copy.top(screen.scrollback());
        let (from, to) = copy.selected();
        for row in 0..inner.height {
            let line = top + usize::from(row);
            if (from..=to).contains(&line) {
                let rect = Rect { y: inner.y + row, height: 1, ..inner };
                frame.buffer_mut().set_style(rect, Style::new().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

fn overview_lines(host: &Host) -> Vec<Line<'static>> {
    let key = |k: &'static str, desc: &'static str| {
        Line::from(vec![format!("{k:<7}").fg(ACCENT).bold(), desc.into()])
    };
    vec![
        Line::from(vec![host.data.alias.clone().bold(), " — ".dim(), host.data.target().into()]),
        Line::default(),
        key("Space", "open a session here"),
        key("Alt-v", "open it in a new column"),
        key("f", "full-screen ssh"),
        key("s", "sftp in a new column"),
        key("c", "install your public key (ssh-copy-id)"),
        key("Enter", "edit (also e)"),
    ]
}

fn history_lines(app: &App) -> Vec<Line<'static>> {
    if app.history.is_empty() {
        return Vec::new();
    }
    let now = db::now();
    let mut lines = vec![Line::from("Recent connections".bold().fg(ACCENT))];
    for entry in &app.history {
        let args: Vec<String> = entry.args.iter().map(|a| shell_quote(a)).collect();
        lines.push(Line::from(vec![
            format!("{:<12}", relative_time(now - entry.connected_at)).dim(),
            format!("sshh {}", args.join(" ")).into(),
        ]));
    }
    lines
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(status) = &app.status {
        let color = if status.error { Color::Red } else { Color::Green };
        frame.render_widget(Line::from(format!(" {}", status.text)).fg(color), area);
        return;
    }
    let active = app.active_key().and_then(|key| app.sessions.get(&key).copied());
    let keys: &[(&str, &str)] = match (&app.mode, app.focus, active) {
        (Mode::Search, ..) => &[("", "type to filter"), ("↑↓ Ctrl-j/k", "move"), ("Enter/Esc", "back to list")],
        (_, Focus::Terminal, Some(_))
            if app.active_key().is_some_and(|key| app.history_sessions.contains(&key)) =>
        {
            &[
                ("j/k", "move"),
                ("/", "search"),
                ("n/N", "prev/next match"),
                ("v", "select lines"),
                ("y", "copy"),
                ("q/Esc", "exit"),
            ]
        }
        (_, Focus::Terminal, Some(SessionState::Alive)) => &[
            ("Alt-hjkl", "move"),
            ("Alt-v/-", "split"),
            ("Alt-J/K", "stack"),
            ("Alt-w", "close"),
            ("Alt-f", "full screen"),
            ("Alt-s", "history"),
            ("Alt-n/p", "session"),
            ("", "other keys go to ssh"),
        ],
        (_, Focus::Terminal, _) => {
            &[("Enter", "connect"), ("Esc Alt-h", "back"), ("Alt-w", "close pane")]
        }
        (_, Focus::Detail, _) => &[("j/k", "scroll"), ("Tab/S-Tab", "next/prev pane"), ("Esc", "list"), ("q", "quit")],
        _ if app.tab == ListTab::Tags => &[
            ("Space", "filter by tag"),
            ("j/k", "move"),
            ("[ ]", "tabs"),
            ("Esc", "connections"),
            ("?", "help"),
            ("q", "quit"),
        ],
        _ => &[
            ("Space", "connect"),
            ("Alt-v/-", "split"),
            ("Alt-l", "panes"),
            ("/", "search"),
            ("a", "add"),
            ("Enter/e", "edit"),
            ("[ ]", "tags"),
            ("x", "close session"),
            ("f", "full screen"),
            ("?", "help"),
            ("q", "quit"),
        ],
    };
    let mut spans: Vec<Span> = keys
        .iter()
        .flat_map(|(k, desc)| [format!(" {k} ").fg(ACCENT).bold(), format!("{desc} ").dim()])
        .collect();
    if let Some(c) = app.pending {
        spans.insert(0, format!(" {c}… ").fg(Color::Yellow).bold());
    }
    frame.render_widget(Line::from(spans), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [area] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(area);
    area
}

fn draw_confirm(frame: &mut Frame, question: Vec<Span<'_>>, action: &str) {
    let lines = vec![
        Line::from(question),
        Line::default(),
        Line::from(vec![
            " y ".fg(Color::Red).bold(),
            format!("{action}   ").dim(),
            " any other key ".fg(ACCENT).bold(),
            "cancel".dim(),
        ]),
    ];
    let area = centered(frame.area(), 56, 5);
    frame.render_widget(Clear, area);
    let block = block("Confirm", true).border_style(Style::new().fg(Color::Red));
    frame.render_widget(Paragraph::new(lines).centered().block(block), area);
}

fn draw_help(frame: &mut Frame) {
    const HELP: &[(&str, &str)] = &[
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
        ("x", "close the connection's sessions (ssh and sftp)"),
        ("f", "full-screen ssh (back to sshh on exit)"),
        ("", ""),
        ("j/k ↑/↓", "move"),
        ("gg / G", "go to top / bottom"),
        ("Ctrl-d/u", "half page down / up"),
        ("Ctrl-f/b", "page down / up"),
        ("Tab / S-Tab", "next / previous pane (list, details, terminal)"),
        ("/", "search (filters while typing; #tag by tag)"),
        ("[ / ]", "tabs: connections / tags (Space on a tag filters)"),
        ("Esc", "back / clear search"),
        ("", ""),
        ("a", "add connection"),
        ("Enter / e, t", "edit / edit tags"),
        ("dd", "delete (asks for confirmation)"),
        ("yy", "copy the ssh command"),
        ("s", "sftp in a new column, next to the ssh one"),
        ("c", "install your public key (ssh-copy-id)"),
        ("o", "change sort (recent / most used / alphabetical)"),
        ("R", "reload"),
        ("q / Ctrl-c", "quit"),
        ("", ""),
        ("Click", "select · double click connects"),
        ("Wheel", "move · scroll details or session history"),
    ];
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, desc)| Line::from(vec![format!(" {k:<16}").fg(ACCENT).bold(), (*desc).into()]))
        .collect();
    let area = centered(frame.area(), 72, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block("Help", true)), area);
}
