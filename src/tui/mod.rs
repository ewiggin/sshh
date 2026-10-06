//! TUI: `sshh` without arguments (connection list + embedded ssh sessions),
//! and the wizard for new connections.

mod app;
mod form;
mod session;
mod term;
mod ui;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::Stylize;
use ratatui::text::Line;

use crate::db::Db;
use crate::model::{Host, HostData};
use crate::{connect, include};
use app::{App, External, Focus, Mode, Outcome, Request, SessionState};
use form::{Form, FormEvent, FormKind};
use session::Session;

/// Embedded sessions by connection id.
type Sessions = HashMap<i64, Session>;

pub fn run() -> Result<()> {
    let mut db = Db::open_default()?;
    let mut app = App::new(db.list_hosts()?);
    let mut sessions = Sessions::new();
    let mut terminal = term::init()?;
    let outcome = event_loop(&mut terminal, &mut db, &mut app, &mut sessions);
    term::restore();
    // Dropping the sessions hangs up the ssh processes still running.
    drop(sessions);
    match outcome? {
        Outcome::Quit => Ok(()),
    }
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    db: &mut Db,
    app: &mut App,
    sessions: &mut Sessions,
) -> Result<Outcome> {
    // Set by the session reader threads when new output arrives.
    let dirty = Arc::new(AtomicBool::new(false));
    let mut redraw = true;
    load_history(db, app)?;
    loop {
        if redraw || dirty.swap(false, Ordering::AcqRel) {
            sync_sessions(sessions, app);
            terminal.draw(|frame| ui::draw(frame, app, sessions))?;
            redraw = resize_selected(sessions, app);
        }
        let timeout = if sessions.is_empty() { Duration::from_secs(1) } else { Duration::from_millis(15) };
        if !event::poll(timeout)? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
            Event::Mouse(mouse) => app.on_mouse(mouse),
            Event::Paste(text) => app.on_paste(&text),
            _ => {}
        }
        redraw = true;
        if let Some(request) = app.request.take() {
            handle_request(terminal, db, app, sessions, &dirty, request)?;
        }
        if let Some(outcome) = app.outcome.take() {
            return Ok(outcome);
        }
        load_history(db, app)?;
    }
}

/// Detects finished sessions and publishes the session states to the app.
fn sync_sessions(sessions: &mut Sessions, app: &mut App) {
    for (id, session) in sessions.iter_mut() {
        if session.poll_exit()
            && let Some(host) = app.hosts.iter().find(|h| h.id == *id)
        {
            app.info(format!("Session '{}': {}", host.data.alias, session.exit().unwrap_or_default()));
        }
    }
    app.sessions = sessions
        .iter()
        .map(|(id, s)| (*id, if s.exit().is_none() { SessionState::Alive } else { SessionState::Ended }))
        .collect();
    if app.focus == Focus::Terminal && app.selected_session().is_none() {
        app.focus = Focus::List;
    }
}

/// Fits the visible session to the right pane. Returns true if it changed.
fn resize_selected(sessions: &mut Sessions, app: &App) -> bool {
    let area = app.terminal_area;
    let Some(session) = app.selected_host().and_then(|h| sessions.get_mut(&h.id)) else {
        return false;
    };
    let before = session.parser().screen().size();
    session.resize(area.height, area.width);
    before != session.parser().screen().size()
}

/// Loads the history of the selected connection if it changed.
fn load_history(db: &Db, app: &mut App) -> Result<()> {
    let selected = app.selected_host().map(|h| h.id);
    if selected != app.history_for {
        app.history = match selected {
            Some(id) => db.history(Some(id), 5)?,
            None => Vec::new(),
        };
        app.history_for = selected;
    }
    Ok(())
}

/// Reloads the list after a connection was used (last use, use count).
fn reload_hosts(db: &Db, app: &mut App) -> Result<()> {
    let selected = app.selected_host().map(|h| h.id);
    app.set_hosts(db.list_hosts()?, selected);
    app.history_for = None;
    Ok(())
}

fn open_session(
    db: &Db,
    app: &mut App,
    sessions: &mut Sessions,
    dirty: &Arc<AtomicBool>,
    host: &Host,
) -> Result<()> {
    let area = app.terminal_area;
    let spawned = connect::session_command(db, host)
        .and_then(|(ssh, args)| Session::spawn(&ssh, &args, area.height, area.width, Arc::clone(dirty)));
    match spawned {
        Ok(session) => {
            // Replaces a finished session of the same connection, if any.
            sessions.insert(host.id, session);
            reload_hosts(db, app)?;
        }
        Err(e) => {
            app.focus = Focus::List;
            app.error(format!("Could not open a session: {e:#}"));
        }
    }
    Ok(())
}

/// Runs sftp, ssh-copy-id or a full-screen ssh, then comes back to the TUI.
fn run_external(
    terminal: &mut DefaultTerminal,
    db: &Db,
    app: &mut App,
    program: External,
    host: &Host,
) -> Result<()> {
    let (bin, args) = match program {
        External::Ssh => match connect::session_command(db, host) {
            Ok((ssh, args)) => (ssh.into_os_string(), args),
            Err(e) => {
                app.error(format!("{e:#}"));
                return Ok(());
            }
        },
        External::Sftp => ("sftp".into(), connect::tool_args(host)),
        External::SshCopyId => {
            let mut args = Vec::new();
            if let Some(identity) = &host.data.identity_file {
                // ssh-copy-id doesn't expand `~` (ssh does for IdentityFile).
                let identity = crate::ssh_config::expand_home(identity);
                args.extend(["-i".to_string(), identity.to_string_lossy().into_owned()]);
            }
            args.extend(connect::tool_args(host));
            ("ssh-copy-id".into(), args)
        }
    };
    let name = bin.to_string_lossy().rsplit('/').next().unwrap_or_default().to_string();
    match term::run_external(terminal, &bin, &args) {
        Ok(status) if status.success() => app.info(format!("{name} finished")),
        Ok(status) => app.error(format!("{name} exited with {status}")),
        Err(e) => app.error(format!("{e:#}")),
    }
    if program == External::Ssh {
        reload_hosts(db, app)?;
    }
    Ok(())
}

fn handle_request(
    terminal: &mut DefaultTerminal,
    db: &mut Db,
    app: &mut App,
    sessions: &mut Sessions,
    dirty: &Arc<AtomicBool>,
    request: Request,
) -> Result<()> {
    let selected = app.selected_host().map(|h| h.id);
    match request {
        Request::Save { id, data } => {
            let saved = match id {
                Some(id) => db.update_host(id, &data).map(|()| id),
                None => db.insert_host(&data),
            };
            let result = saved.and_then(|id| Ok((id, db.list_hosts()?)));
            let ok = result.is_ok();
            app.on_saved(result.map_err(|e| format!("{e:#}")));
            if ok {
                refresh_include(db, app);
            }
        }
        Request::Delete { alias } => {
            let id = app.hosts.iter().find(|h| h.data.alias == alias).map(|h| h.id);
            match db.delete_by_alias(&alias) {
                Ok(_) => {
                    if let Some(id) = id {
                        sessions.remove(&id);
                    }
                    app.set_hosts(db.list_hosts()?, None);
                    app.info(format!("Connection '{alias}' deleted"));
                    refresh_include(db, app);
                }
                Err(e) => app.error(format!("Could not delete: {e:#}")),
            }
        }
        Request::Copy(data) => {
            let cmd = connect::ssh_command(&data);
            match term::copy(&cmd) {
                Ok(()) => app.info(format!("Copied: {cmd}")),
                Err(e) => app.error(format!("Could not copy: {e:#}")),
            }
        }
        Request::Editor => {
            if let Mode::Form(form) = &mut app.mode {
                edit_focused(terminal, form);
            }
        }
        Request::Reload => {
            reload_hosts(db, app)?;
            app.info("List reloaded");
        }
        Request::OpenSession(host) => open_session(db, app, sessions, dirty, &host)?,
        Request::CloseSession(id) => {
            if sessions.remove(&id).is_some() {
                app.info("Session closed");
            }
        }
        Request::Input(key) => {
            if let Some(session) = selected.and_then(|id| sessions.get_mut(&id)) {
                session.send_key(key);
            }
        }
        Request::Paste(text) => {
            if let Some(session) = selected.and_then(|id| sessions.get_mut(&id)) {
                session.paste(&text);
            }
        }
        Request::Scroll(delta) => {
            if let Some(session) = selected.and_then(|id| sessions.get_mut(&id)) {
                session.scroll(delta);
            }
        }
        Request::External { program, host } => run_external(terminal, db, app, program, &host)?,
    }
    Ok(())
}

fn refresh_include(db: &Db, app: &mut App) {
    if let Err(e) = include::refresh(db) {
        app.error(format!("Could not update ~/.ssh/config.d/sshh.conf: {e:#}"));
    }
}

fn edit_focused(terminal: &mut DefaultTerminal, form: &mut Form) {
    let text = form.focused().value.clone();
    match term::edit_external(terminal, &text) {
        Ok(new) => form.set_focused_value(new.trim_end().to_string()),
        Err(e) => form.error = Some(format!("{e:#}")),
    }
}

pub enum WizardOutcome {
    Saved(i64),
    /// Connect without saving.
    Skipped,
    /// Ctrl-c: don't connect.
    Aborted,
}

/// Form to save a new connection before connecting.
pub fn wizard(db: &mut Db, prefill: &HostData, destination: &str) -> Result<WizardOutcome> {
    let mut form = Form::new(FormKind::Wizard, prefill);
    let mut terminal = term::init()?;
    let outcome = wizard_loop(&mut terminal, db, &mut form, destination);
    term::restore();
    outcome
}

fn wizard_loop(
    terminal: &mut DefaultTerminal,
    db: &mut Db,
    form: &mut Form,
    destination: &str,
) -> Result<WizardOutcome> {
    loop {
        terminal.draw(|frame| {
            let [header, _] =
                Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(frame.area());
            let text = Line::from(vec![
                " sshh: ".bold(),
                destination.to_string().fg(ui::ACCENT).bold(),
                " is not saved. Save it before connecting?".into(),
            ]);
            frame.render_widget(text, header);
            form.render(frame);
        })?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return Ok(WizardOutcome::Aborted);
                }
                match form.on_key(key) {
                    FormEvent::None => {}
                    FormEvent::Cancel => return Ok(WizardOutcome::Skipped),
                    FormEvent::Editor => edit_focused(terminal, form),
                    FormEvent::Submit => {
                        if let Some(data) = form.submit() {
                            match db.insert_host(&data) {
                                Ok(id) => return Ok(WizardOutcome::Saved(id)),
                                Err(e) => form.error = Some(format!("{e:#}")),
                            }
                        }
                    }
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                form.on_click(Position::new(mouse.column, mouse.row));
            }
            _ => {}
        }
    }
}
