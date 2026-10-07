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

use super::app::{App, Focus, Mode, SessionState};
use super::session::Session;
use crate::cli::{format_date, relative_time};
use crate::connect::shell_quote;
use crate::db;
use crate::model::Host;

pub const ACCENT: Color = Color::Cyan;
const MATCH: Color = Color::Yellow;
const TAG: Color = Color::Magenta;
const CONNECTED: Color = Color::Green;

pub fn draw(frame: &mut Frame, app: &mut App, sessions: &HashMap<i64, Session>) {
    let [main, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    app.column_areas.clear();
    if app.zoomed && !app.columns.is_empty() {
        // Only the active column, on the whole screen.
        app.rows_area = Rect::default();
        app.search_area = Rect::default();
        app.detail_area = Rect::default();
        app.columns_area = main;
        draw_column(frame, app, sessions, app.active_column, main);
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

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = format!("Connections {}/{} · {}", app.entries.len(), app.hosts.len(), app.sort.label());
    let block = block(&title, pane_focused(app, Focus::List));
    app.rows_area = block.inner(area);

    if app.entries.is_empty() {
        let msg = if app.hosts.is_empty() { "No connections yet.\nPress 'a' to add one." } else { "No results." };
        let p = Paragraph::new(msg).dim().centered().wrap(Wrap { trim: false });
        let [middle] = Layout::vertical([Constraint::Length(2)]).flex(Flex::Center).areas(app.rows_area);
        frame.render_widget(block, area);
        frame.render_widget(p, middle);
        return;
    }

    let rows = app.entries.iter().map(|e| {
        let host = &app.hosts[e.index];
        let d = &host.data;
        let mut spans = highlighted(&d.alias, &e.alias_hl);
        if let Some(name) = &d.name {
            spans.push(Span::raw("  "));
            spans.extend(highlighted(name, &e.name_hl).into_iter().map(|s| s.dim()));
        }
        Row::new([Cell::from(session_dot(app.sessions.get(&host.id).copied())), Cell::from(Line::from(spans))])
    });
    let table = Table::new(rows, [Constraint::Length(1), Constraint::Fill(1)])
        .block(block)
        .column_spacing(1)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_spacing(HighlightSpacing::Never);
    frame.render_stateful_widget(table, area, &mut app.table);
}

fn tags_line(host: &Host) -> Line<'static> {
    Line::from(host.data.tags.iter().map(|t| format!("#{t} ").fg(TAG)).collect::<Vec<_>>())
}

fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    app.detail_area = area;
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

/// Right side: the terminal columns, or an overview of the selected
/// connection when no column is open.
fn draw_columns(frame: &mut Frame, app: &mut App, sessions: &HashMap<i64, Session>, area: Rect) {
    app.columns_area = area;
    if app.columns.is_empty() {
        draw_overview(frame, app, sessions, area);
        return;
    }
    let areas = Layout::horizontal(vec![Constraint::Fill(1); app.columns.len()]).split(area);
    for (index, column) in areas.iter().enumerate() {
        draw_column(frame, app, sessions, index, *column);
    }
}

fn draw_overview(frame: &mut Frame, app: &App, sessions: &HashMap<i64, Session>, area: Rect) {
    let Some(host) = app.selected_host() else {
        let text = if app.hosts.is_empty() {
            "No saved connections.\n\nPress 'a' to add one, or import your ~/.ssh/config\nwith `sshh import-ssh-config`."
        } else {
            "No connection selected."
        };
        let block = block("sshh", false);
        let [middle] = Layout::vertical([Constraint::Length(4)]).flex(Flex::Center).areas(block.inner(area));
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(text).dim().centered(), middle);
        return;
    };
    let title = match sessions.get(&host.id).map(|s| s.exit().is_none()) {
        Some(true) => format!("{} — session running · Space shows it", host.data.alias),
        Some(false) => format!("{} — session ended · Space reconnects", host.data.alias),
        None => format!("{} — not connected", host.data.alias),
    };
    frame.render_widget(overview(app, host).block(block(&title, false)), area);
}

/// One terminal column: the session of the connection it shows.
fn draw_column(
    frame: &mut Frame,
    app: &mut App,
    sessions: &HashMap<i64, Session>,
    index: usize,
    area: Rect,
) {
    app.column_areas.push((index, Block::bordered().inner(area)));
    let focused = pane_focused(app, Focus::Terminal) && index == app.active_column;
    let id = app.columns[index];
    let Some(host) = app.host(id) else { return };
    let number = index + 1;

    let Some(session) = sessions.get(&id) else {
        let title = format!("{number} {} — no session · Enter connects", host.data.alias);
        let text = vec![Line::default(), Line::from(vec!["  ".into(), host.data.target().bold()])];
        frame.render_widget(Paragraph::new(text).block(block(&title, focused)), area);
        return;
    };

    let parser = session.parser();
    let screen = parser.screen();
    let mut title = match session.exit() {
        None => format!("{number} ● {} — {}", host.data.alias, host.data.target()),
        Some(exit) => format!("{number} {} — {exit} · Enter reconnects", host.data.alias),
    };
    if screen.scrollback() > 0 {
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
    let show_cursor =
        focused && session.exit().is_none() && !screen.hide_cursor() && screen.scrollback() == 0;
    let terminal = PseudoTerminal::new(screen)
        .block(block)
        .cursor(Cursor::default().visibility(show_cursor));
    frame.render_widget(terminal, area);
}

fn overview<'a>(app: &App, host: &'a Host) -> Paragraph<'a> {
    let now = db::now();
    let key = |k: &'static str, desc: &'static str| {
        Line::from(vec![format!("  {k:<7}").fg(ACCENT).bold(), desc.into()])
    };
    let mut lines = vec![
        Line::default(),
        Line::from(vec!["  ".into(), host.data.target().bold()]),
        Line::default(),
        key("Space", "open a session here"),
        key("Alt-v", "open it in a new column"),
        key("f", "full-screen ssh"),
        key("s", "sftp"),
        key("c", "install your public key (ssh-copy-id)"),
        key("Enter", "edit (also e)"),
    ];
    if !app.history.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from("  Recent connections".bold().fg(ACCENT)));
        for entry in &app.history {
            let args: Vec<String> = entry.args.iter().map(|a| shell_quote(a)).collect();
            lines.push(Line::from(vec![
                format!("  {:<12}", relative_time(now - entry.connected_at)).dim(),
                format!("sshh {}", args.join(" ")).into(),
            ]));
        }
    }
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(status) = &app.status {
        let color = if status.error { Color::Red } else { Color::Green };
        frame.render_widget(Line::from(format!(" {}", status.text)).fg(color), area);
        return;
    }
    let active = app.active_host_id().and_then(|id| app.sessions.get(&id).copied());
    let keys: &[(&str, &str)] = match (&app.mode, app.focus, active) {
        (Mode::Search, ..) => &[("", "type to filter"), ("↑↓ Ctrl-j/k", "move"), ("Enter/Esc", "back to list")],
        (_, Focus::Terminal, Some(SessionState::Alive)) => &[
            ("Alt-←/→", "column"),
            ("Alt-1..9", "jump"),
            ("Alt-v", "split"),
            ("Alt-w", "close column"),
            ("Alt-f", "full screen"),
            ("Alt-j/k", "session"),
            ("", "other keys go to ssh"),
        ],
        (_, Focus::Terminal, _) => {
            &[("Enter", "connect"), ("Esc Alt-h", "back"), ("Alt-w", "close column")]
        }
        (_, Focus::Detail, _) => &[("j/k", "scroll"), ("Tab/S-Tab", "next/prev pane"), ("Esc", "list"), ("q", "quit")],
        _ => &[
            ("Space", "connect"),
            ("Alt-v", "new column"),
            ("Alt-l", "columns"),
            ("/", "search"),
            ("a", "add"),
            ("Enter/e", "edit"),
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
        ("Space", "show the connection in the active column"),
        ("Alt-v", "show it in a new column (split)"),
        ("Alt-←/→ Alt-h/l", "column left / right (the list is the first)"),
        ("Alt-1 … Alt-9", "jump to column N"),
        ("Alt-j / Alt-k", "next / previous session in the column"),
        ("Alt-w", "close the column (the session keeps running)"),
        ("Alt-f / Alt-z", "full screen for the active column (toggle)"),
        ("Alt-H / Alt-L", "move the column left / right"),
        ("x", "close the session"),
        ("f", "full-screen ssh (back to sshh on exit)"),
        ("", ""),
        ("j/k ↑/↓", "move"),
        ("gg / G", "go to top / bottom"),
        ("Ctrl-d/u", "half page down / up"),
        ("Ctrl-f/b", "page down / up"),
        ("Tab / S-Tab", "next / previous pane (list, details, terminal)"),
        ("/", "search (filters while typing; #tag by tag)"),
        ("Esc", "back / clear search"),
        ("", ""),
        ("a", "add connection"),
        ("Enter / e, t", "edit / edit tags"),
        ("dd", "delete (asks for confirmation)"),
        ("yy", "copy the ssh command"),
        ("s / c", "sftp / install your public key (ssh-copy-id)"),
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
