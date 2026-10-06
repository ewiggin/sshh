//! Formulario de conexión: alta, edición y wizard de conexiones nuevas.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::ui::{ACCENT, block};
use crate::model::{HostData, SshOption, validate_alias};
use crate::ssh_args::parse_option;

pub const ALIAS: usize = 0;
pub const HOSTNAME: usize = 1;
pub const USER: usize = 2;
pub const PORT: usize = 3;
pub const IDENTITY: usize = 4;
pub const JUMP: usize = 5;
pub const NAME: usize = 6;
pub const DESCRIPTION: usize = 7;
pub const TAGS: usize = 8;
pub const OPTIONS: usize = 9;
pub const NOTES: usize = 10;

/// (etiqueta, pista cuando está vacío, líneas; >1 = multilínea)
const FIELDS: [(&str, &str, u16); 11] = [
    ("Alias", "nombre corto para usar con: sshh <alias>", 1),
    ("Host", "hostname o IP", 1),
    ("Usuario", "", 1),
    ("Puerto", "22 por defecto", 1),
    ("Identidad", "p. ej. ~/.ssh/id_ed25519", 1),
    ("ProxyJump", "p. ej. bastion o user@host:port", 1),
    ("Nombre", "", 1),
    ("Descripción", "", 1),
    ("Tags", "p. ej. prod, web", 1),
    ("Opciones", "una por línea, p. ej. ForwardAgent=yes", 3),
    ("Notas", "Ctrl-e abre $EDITOR", 5),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormKind {
    Add,
    Edit(i64),
    /// Conexión nueva detectada al usar `sshh destino`.
    Wizard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormEvent {
    None,
    Submit,
    Cancel,
    /// Editar el campo multilínea enfocado con $EDITOR.
    Editor,
}

#[derive(Debug, Clone, Default)]
pub struct Field {
    pub value: String,
    /// Posición del cursor en chars.
    pub cursor: usize,
}

impl Field {
    fn new(value: String) -> Self {
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    fn byte(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map_or(self.value.len(), |(b, _)| b)
    }

    /// (línea, columna) del cursor.
    fn line_col(&self) -> (usize, usize) {
        let before: String = self.value.chars().take(self.cursor).collect();
        let line = before.matches('\n').count();
        let col = before.rsplit('\n').next().unwrap_or_default().chars().count();
        (line, col)
    }

    /// Mueve el cursor a (línea, columna), ajustando la columna a la línea.
    fn set_line_col(&mut self, line: usize, col: usize) {
        let mut cursor = 0;
        for (i, l) in self.value.split('\n').enumerate() {
            let len = l.chars().count();
            if i == line {
                self.cursor = cursor + col.min(len);
                return;
            }
            cursor += len + 1;
        }
    }

    fn insert(&mut self, c: char) {
        let b = self.byte(self.cursor);
        self.value.insert(b, c);
        self.cursor += 1;
    }

    fn delete_range(&mut self, from: usize, to: usize) {
        let (a, b) = (self.byte(from), self.byte(to));
        self.value.replace_range(a..b, "");
        self.cursor = from;
    }

    fn line_start(&self) -> usize {
        let (_, col) = self.line_col();
        self.cursor - col
    }

    fn line_end(&self) -> usize {
        let rest = self.value.chars().skip(self.cursor);
        self.cursor + rest.take_while(|&c| c != '\n').count()
    }

    fn word_start(&self) -> usize {
        let chars: Vec<char> = self.value.chars().take(self.cursor).collect();
        let mut i = chars.len();
        while i > 0 && chars[i - 1].is_whitespace() && chars[i - 1] != '\n' {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }
}

pub struct Form {
    pub kind: FormKind,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
    /// Zonas de los campos en el último render (para el ratón).
    areas: Vec<Rect>,
}

fn opt(s: &Option<String>) -> String {
    s.clone().unwrap_or_default()
}

impl Form {
    pub fn new(kind: FormKind, data: &HostData) -> Self {
        let mut fields = vec![Field::default(); FIELDS.len()];
        fields[ALIAS] = Field::new(data.alias.clone());
        fields[HOSTNAME] = Field::new(data.hostname.clone());
        fields[USER] = Field::new(opt(&data.user));
        fields[PORT] = Field::new(data.port.map(|p| p.to_string()).unwrap_or_default());
        fields[IDENTITY] = Field::new(opt(&data.identity_file));
        fields[JUMP] = Field::new(opt(&data.proxy_jump));
        fields[NAME] = Field::new(opt(&data.name));
        fields[DESCRIPTION] = Field::new(opt(&data.description));
        fields[TAGS] = Field::new(data.tags.join(", "));
        let options: Vec<String> =
            data.extra_options.iter().map(|o| format!("{}={}", o.key, o.value)).collect();
        fields[OPTIONS] = Field::new(options.join("\n"));
        fields[NOTES] = Field::new(opt(&data.notes));
        Self { kind, fields, focus: ALIAS, error: None, areas: Vec::new() }
    }

    pub fn focus(mut self, field: usize) -> Self {
        self.focus = field;
        self
    }

    pub fn is_multiline(field: usize) -> bool {
        FIELDS[field].2 > 1
    }

    /// Convierte el formulario en datos validados. En caso de error enfoca el
    /// campo culpable y guarda el mensaje.
    pub fn submit(&mut self) -> Option<HostData> {
        match self.build() {
            Ok(data) => {
                self.error = None;
                Some(data)
            }
            Err((field, msg)) => {
                self.focus = field;
                self.error = Some(msg);
                None
            }
        }
    }

    fn build(&self) -> Result<HostData, (usize, String)> {
        let text = |f: usize| self.fields[f].value.trim().to_string();
        let optional = |f: usize| Some(text(f)).filter(|s| !s.is_empty());

        let alias = text(ALIAS);
        validate_alias(&alias).map_err(|e| (ALIAS, e.to_string()))?;
        let hostname = text(HOSTNAME);
        if hostname.is_empty() || hostname.chars().any(char::is_whitespace) {
            return Err((HOSTNAME, "el host no puede estar vacío ni tener espacios".into()));
        }
        let port = match optional(PORT) {
            None => None,
            Some(p) => match p.parse::<u16>() {
                Ok(n) if n > 0 => Some(n),
                _ => return Err((PORT, format!("puerto inválido «{p}»"))),
            },
        };
        let tags = text(TAGS)
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(|t| t.trim_start_matches('#'))
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect();
        let mut extra_options = Vec::new();
        for line in self.fields[OPTIONS].value.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (key, value) = parse_option(line)
                .ok_or_else(|| (OPTIONS, format!("se esperaba Clave=Valor, no «{line}»")))?;
            extra_options.push(SshOption { key, value });
        }
        let notes = self.fields[NOTES].value.trim_end().to_string();

        Ok(HostData {
            alias,
            hostname,
            user: optional(USER),
            port,
            identity_file: optional(IDENTITY),
            proxy_jump: optional(JUMP),
            extra_options,
            name: optional(NAME),
            description: optional(DESCRIPTION),
            notes: Some(notes).filter(|n| !n.is_empty()),
            tags,
        })
    }

    pub fn set_focused_value(&mut self, value: String) {
        self.fields[self.focus] = Field::new(value);
    }

    pub fn focused(&self) -> &Field {
        &self.fields[self.focus]
    }

    fn move_focus(&mut self, delta: isize) {
        let n = self.fields.len() as isize;
        self.focus = (self.focus as isize + delta).rem_euclid(n) as usize;
    }

    pub fn on_key(&mut self, key: KeyEvent) -> FormEvent {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let multiline = Self::is_multiline(self.focus);
        let field = &mut self.fields[self.focus];
        match key.code {
            KeyCode::Esc => return FormEvent::Cancel,
            KeyCode::Char('c') if ctrl => return FormEvent::Cancel,
            KeyCode::Char('s') if ctrl => return FormEvent::Submit,
            KeyCode::Char('e') if ctrl && multiline => return FormEvent::Editor,
            KeyCode::Enter if multiline => field.insert('\n'),
            KeyCode::Enter => return FormEvent::Submit,
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Up | KeyCode::Down => {
                let down = key.code == KeyCode::Down;
                let (line, col) = field.line_col();
                let lines = field.value.split('\n').count();
                match (multiline, down) {
                    (true, false) if line > 0 => field.set_line_col(line - 1, col),
                    (true, true) if line + 1 < lines => field.set_line_col(line + 1, col),
                    _ => self.move_focus(if down { 1 } else { -1 }),
                }
            }
            KeyCode::Left => field.cursor = field.cursor.saturating_sub(1),
            KeyCode::Right => field.cursor = (field.cursor + 1).min(field.value.chars().count()),
            KeyCode::Home => field.cursor = field.line_start(),
            KeyCode::Char('a') if ctrl => field.cursor = field.line_start(),
            KeyCode::End => field.cursor = field.line_end(),
            KeyCode::Backspace if field.cursor > 0 => {
                field.delete_range(field.cursor - 1, field.cursor);
            }
            KeyCode::Delete if field.cursor < field.value.chars().count() => {
                field.delete_range(field.cursor, field.cursor + 1);
            }
            KeyCode::Char('w') if ctrl => field.delete_range(field.word_start(), field.cursor),
            KeyCode::Char('u') if ctrl => field.delete_range(field.line_start(), field.cursor),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                field.insert(c);
            }
            _ => {}
        }
        FormEvent::None
    }

    /// Click: enfoca el campo bajo el ratón.
    pub fn on_click(&mut self, pos: Position) {
        if let Some(i) = self.areas.iter().position(|a| a.contains(pos)) {
            self.focus = i;
            self.fields[i].cursor = self.fields[i].value.chars().count();
        }
    }

    fn title(&self) -> String {
        match self.kind {
            FormKind::Add => "Nueva conexión".into(),
            FormKind::Edit(_) => format!("Editar «{}»", self.fields[ALIAS].value),
            FormKind::Wizard => "Conexión nueva · ¿guardarla?".into(),
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut hints = vec![];
        if Self::is_multiline(self.focus) {
            hints.extend([("Ctrl-s", "guardar"), ("Ctrl-e", "$EDITOR")]);
        } else {
            hints.push(("Enter", "guardar"));
        }
        hints.push(("Tab/↑↓", "campo"));
        hints.push(match self.kind {
            FormKind::Wizard => ("Esc", "conectar sin guardar"),
            _ => ("Esc", "cancelar"),
        });
        hints
    }

    pub fn render(&mut self, frame: &mut Frame) {
        const LABEL: u16 = 13;
        let heights: Vec<u16> = FIELDS.iter().map(|f| f.2).collect();
        let height = heights.iter().sum::<u16>() + 4;
        let width = frame.area().width.saturating_sub(4).min(76);
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(frame.area());
        let [area] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(area);
        frame.render_widget(Clear, area);
        let title = self.title();
        let block = block(&title, true);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut constraints: Vec<Constraint> = heights.iter().map(|&h| Constraint::Length(h)).collect();
        constraints.extend([Constraint::Length(1), Constraint::Length(1)]);
        let rows = Layout::vertical(constraints).split(inner);
        self.areas.clear();

        for (i, (label, hint, height)) in FIELDS.iter().enumerate() {
            let focused = i == self.focus;
            let [label_area, input] =
                Layout::horizontal([Constraint::Length(LABEL), Constraint::Fill(1)]).areas(rows[i]);
            self.areas.push(rows[i]);
            let label_style = if focused { Style::new().fg(ACCENT).bold() } else { Style::new().dim() };
            let marker = if focused { "› " } else { "  " };
            frame.render_widget(Span::styled(format!("{marker}{label}"), label_style), label_area);

            let field = &self.fields[i];
            let bg = if focused { Style::new().bg(Color::Indexed(236)) } else { Style::new() };
            if field.value.is_empty() {
                frame.render_widget(Paragraph::new(hint.dim()).style(bg), input);
            } else {
                let (line, col) = field.line_col();
                let (scroll_y, scroll_x) = (
                    line.saturating_sub(usize::from(*height) - 1) as u16,
                    col.saturating_sub(usize::from(input.width.saturating_sub(1))) as u16,
                );
                let scroll_x = if focused { scroll_x } else { 0 };
                let p = Paragraph::new(field.value.as_str()).style(bg).scroll((scroll_y, scroll_x));
                frame.render_widget(p, input);
            }
            if focused {
                let (line, col) = field.line_col();
                let y = (line as u16).min(height - 1);
                let x = (col as u16).min(input.width.saturating_sub(1));
                frame.set_cursor_position((input.x + x, input.y + y));
            }
        }

        if let Some(err) = &self.error {
            frame.render_widget(Line::from(format!("✗ {err}")).red(), rows[FIELDS.len()]);
        }
        let hints: Vec<Span> = self
            .hints()
            .into_iter()
            .flat_map(|(k, d)| [format!("{k} ").fg(ACCENT).bold(), format!("{d}  ").dim()])
            .collect();
        frame.render_widget(Line::from(hints), rows[FIELDS.len() + 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_str(form: &mut Form, s: &str) {
        for c in s.chars() {
            let code = if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) };
            form.on_key(key(code));
        }
    }

    #[test]
    fn roundtrip() {
        let data = HostData {
            alias: "web".into(),
            hostname: "10.0.0.1".into(),
            user: Some("root".into()),
            port: Some(2222),
            tags: vec!["a".into(), "b".into()],
            extra_options: vec![SshOption { key: "ForwardAgent".into(), value: "yes".into() }],
            notes: Some("línea 1\nlínea 2".into()),
            ..Default::default()
        };
        let mut form = Form::new(FormKind::Add, &data);
        assert_eq!(form.submit().unwrap(), data);
    }

    #[test]
    fn validation_focuses_field() {
        let mut form = Form::new(FormKind::Add, &HostData { alias: "a".into(), ..Default::default() });
        assert!(form.submit().is_none());
        assert_eq!(form.focus, HOSTNAME);

        form.focus = HOSTNAME;
        type_str(&mut form, "h");
        form.focus = PORT;
        type_str(&mut form, "99999");
        assert!(form.submit().is_none());
        assert_eq!(form.focus, PORT);
        assert!(form.error.as_deref().unwrap().contains("99999"));
    }

    #[test]
    fn tags_are_normalized() {
        let mut form = Form::new(FormKind::Add, &HostData { alias: "a".into(), hostname: "h".into(), ..Default::default() });
        form.focus = TAGS;
        type_str(&mut form, "#prod, web  db,");
        assert_eq!(form.submit().unwrap().tags, ["prod", "web", "db"]);
    }

    #[test]
    fn editing_keys() {
        let mut form = Form::new(FormKind::Add, &HostData::default());
        type_str(&mut form, "hola mundo");
        form.on_key(ctrl('w'));
        assert_eq!(form.focused().value, "hola ");
        form.on_key(key(KeyCode::Home));
        type_str(&mut form, "¡");
        form.on_key(key(KeyCode::End));
        form.on_key(key(KeyCode::Backspace));
        assert_eq!(form.focused().value, "¡hola");
        form.on_key(key(KeyCode::Left));
        form.on_key(key(KeyCode::Delete));
        assert_eq!(form.focused().value, "¡hol");
    }

    #[test]
    fn enter_submits_single_line_and_inserts_newline_in_multiline() {
        let mut form = Form::new(FormKind::Add, &HostData::default());
        assert_eq!(form.on_key(key(KeyCode::Enter)), FormEvent::Submit);
        form.focus = NOTES;
        type_str(&mut form, "a\nbc");
        assert_eq!(form.focused().value, "a\nbc");
        assert_eq!(form.on_key(ctrl('e')), FormEvent::Editor);
        assert_eq!(form.on_key(ctrl('s')), FormEvent::Submit);
    }

    #[test]
    fn multiline_arrows_move_lines_then_fields() {
        let mut form = Form::new(FormKind::Add, &HostData { notes: Some("abc\nd".into()), ..Default::default() });
        form.focus = NOTES;
        assert_eq!(form.focused().line_col(), (1, 1));
        form.on_key(key(KeyCode::Up));
        assert_eq!((form.focus, form.focused().line_col()), (NOTES, (0, 1)));
        form.on_key(key(KeyCode::Up));
        assert_eq!(form.focus, OPTIONS);
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.focus, ALIAS);
    }
}
