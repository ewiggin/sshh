//! TUI: `sshh` without arguments, and the wizard for new connections.

mod app;
mod form;
mod term;
mod ui;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::Stylize;
use ratatui::text::Line;

use crate::db::Db;
use crate::{connect, include};
use crate::model::HostData;
use app::{App, Mode, Outcome, Request};
use form::{Form, FormEvent, FormKind};

pub fn run() -> Result<()> {
    let mut db = Db::open_default()?;
    let mut app = App::new(db.list_hosts()?);
    let mut terminal = term::init()?;
    let outcome = event_loop(&mut terminal, &mut db, &mut app);
    term::restore();
    drop(db);
    match outcome? {
        Outcome::Quit => Ok(()),
        Outcome::Connect(alias) => match connect::run(vec![alias])? {},
        Outcome::Run { program, host } => {
            use std::os::unix::process::CommandExt;
            let mut args = Vec::new();
            if program == "ssh-copy-id"
                && let Some(identity) = &host.data.identity_file
            {
                // ssh-copy-id doesn't expand `~` (ssh does for IdentityFile).
                let identity = crate::ssh_config::expand_home(identity);
                args.extend(["-i".to_string(), identity.to_string_lossy().into_owned()]);
            }
            args.extend(connect::tool_args(&host));
            let err = std::process::Command::new(program).args(&args).exec();
            Err(anyhow::anyhow!(err).context(format!("could not run {program}")))
        }
    }
}

fn event_loop(terminal: &mut DefaultTerminal, db: &mut Db, app: &mut App) -> Result<Outcome> {
    load_history(db, app)?;
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
            Event::Mouse(mouse) => app.on_mouse(mouse),
            _ => {}
        }
        if let Some(request) = app.request.take() {
            handle_request(terminal, db, app, request)?;
        }
        if let Some(outcome) = app.outcome.take() {
            return Ok(outcome);
        }
        load_history(db, app)?;
    }
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

fn handle_request(
    terminal: &mut DefaultTerminal,
    db: &mut Db,
    app: &mut App,
    request: Request,
) -> Result<()> {
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
        Request::Delete { alias } => match db.delete_by_alias(&alias) {
            Ok(_) => {
                app.set_hosts(db.list_hosts()?, None);
                app.info(format!("Connection '{alias}' deleted"));
                refresh_include(db, app);
            }
            Err(e) => app.error(format!("Could not delete: {e:#}")),
        },
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
            let selected = app.selected_host().map(|h| h.id);
            app.set_hosts(db.list_hosts()?, selected);
            app.history_for = None;
            app.info("List reloaded");
        }
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
