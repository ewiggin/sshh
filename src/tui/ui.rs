//! Renderizado del TUI.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, Wrap,
};

use super::app::{App, Focus, Mode};
use crate::cli::{format_date, relative_time};
use crate::connect::shell_quote;
use crate::db;
use crate::model::HostData;

pub const ACCENT: Color = Color::Cyan;
const MATCH: Color = Color::Yellow;
const TAG: Color = Color::Magenta;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [search, main, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let [list, detail] = if main.width >= 100 {
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(main)
    } else {
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(main)
    };

    draw_search(frame, app, search);
    draw_table(frame, app, list);
    draw_detail(frame, app, detail);
    draw_footer(frame, app, footer);
    match &mut app.mode {
        Mode::Help => draw_help(frame),
        Mode::Form(form) => form.render(frame),
        Mode::ConfirmDelete { alias } => draw_confirm(frame, alias),
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

fn draw_search(frame: &mut Frame, app: &mut App, area: Rect) {
    app.search_area = area;
    let searching = matches!(app.mode, Mode::Search);
    let content = if app.query.is_empty() && !searching {
        Line::from("/ para buscar · #tag filtra por tag".dim())
    } else {
        Line::from(app.query.as_str())
    };
    let block = block("Buscar", searching);
    let inner = block.inner(area);
    frame.render_widget(Paragraph::new(content).block(block), area);
    if searching {
        let x = inner.x + (app.query.chars().count() as u16).min(inner.width.saturating_sub(1));
        frame.set_cursor_position((x, inner.y));
    }
}

/// Texto con los chars de `positions` resaltados.
fn highlighted<'a>(text: &'a str, positions: &[usize]) -> Line<'a> {
    if positions.is_empty() {
        return Line::from(text);
    }
    let hl = Style::new().fg(MATCH).bold();
    Line::from(
        text.chars()
            .enumerate()
            .map(|(i, c)| {
                let span = Span::raw(c.to_string());
                if positions.contains(&i) { span.style(hl) } else { span }
            })
            .collect::<Vec<_>>(),
    )
}

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = format!("Conexiones {}/{} · {}", app.entries.len(), app.hosts.len(), app.sort.label());
    let focused = matches!(app.mode, Mode::Normal) && app.focus == Focus::List;
    let block = block(&title, focused);
    let inner = block.inner(area);
    // La primera línea es la cabecera.
    app.rows_area = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };

    if app.entries.is_empty() {
        let msg = if app.hosts.is_empty() {
            "No hay conexiones guardadas.\nPulsa «a» para añadir una o importa tu ~/.ssh/config\ncon `sshh import-ssh-config`."
        } else {
            "Sin resultados."
        };
        let p = Paragraph::new(msg).dim().centered().wrap(Wrap { trim: false });
        let [middle] = Layout::vertical([Constraint::Length(3)]).flex(Flex::Center).areas(inner);
        frame.render_widget(block, area);
        frame.render_widget(p, middle);
        return;
    }

    let now = db::now();
    // Ancho según el contenido (acotado); NOMBRE se queda con el resto.
    let width = |f: &dyn Fn(&HostData) -> usize, min: usize, max: usize| {
        let w = app.entries.iter().map(|e| f(&app.hosts[e.index].data)).max().unwrap_or(0);
        w.clamp(min, max) as u16
    };
    let alias_width = width(&|d| d.alias.chars().count(), 5, 24);
    let target_width = width(&|d| d.target().chars().count(), 7, 40);
    let tags_width = width(&|d| tags_text(d).chars().count(), 4, 24);
    let rows = app.entries.iter().map(|e| {
        let host = &app.hosts[e.index];
        let d = &host.data;
        Row::new([
            Cell::from(highlighted(&d.alias, &e.alias_hl)),
            Cell::from(d.target()),
            Cell::from(highlighted(d.name.as_deref().unwrap_or_default(), &e.name_hl)),
            Cell::from(tags_text(d).fg(TAG)),
            Cell::from(host.last_used.map(|t| relative_time(now - t)).unwrap_or_default().dim()),
        ])
    });
    let header = Row::new(["ALIAS", "DESTINO", "NOMBRE", "TAGS", "ÚLTIMO USO"])
        .style(Style::new().bold().fg(ACCENT));
    let table = Table::new(
        rows,
        [
            Constraint::Length(alias_width),
            Constraint::Length(target_width),
            Constraint::Min(6),
            Constraint::Length(tags_width),
            Constraint::Length(11),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
    .highlight_symbol("› ")
    .highlight_spacing(HighlightSpacing::Always);
    frame.render_stateful_widget(table, area, &mut app.table);
}

fn tags_text(d: &HostData) -> String {
    d.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")
}

fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    app.detail_area = area;
    let focused = matches!(app.mode, Mode::Normal) && app.focus == Focus::Detail;
    let block = block("Detalle", focused);
    let Some(host) = app.selected_host() else {
        frame.render_widget(block, area);
        return;
    };
    let d = &host.data;
    let now = db::now();
    let field = |label: &'static str, value: String| {
        Line::from(vec![format!("{label:<11}").fg(ACCENT), value.into()])
    };

    let mut lines = vec![Line::from(d.name.clone().unwrap_or_else(|| d.alias.clone()).bold())];
    if let Some(desc) = &d.description {
        lines.push(Line::from(desc.clone().italic()));
    }
    lines.push(Line::default());
    lines.push(field("Alias", d.alias.clone()));
    lines.push(field("Destino", d.target()));
    if let Some(identity) = &d.identity_file {
        lines.push(field("Identidad", identity.clone()));
    }
    if let Some(jump) = &d.proxy_jump {
        lines.push(field("ProxyJump", jump.clone()));
    }
    for opt in &d.extra_options {
        lines.push(field("Opción", format!("{} {}", opt.key, opt.value)));
    }
    if !d.tags.is_empty() {
        let mut spans = vec![format!("{:<11}", "Tags").fg(ACCENT)];
        spans.extend(d.tags.iter().map(|t| format!("#{t} ").fg(TAG)));
        lines.push(Line::from(spans));
    }
    let usage = match host.last_used {
        Some(t) => {
            let times = if host.use_count == 1 { "vez" } else { "veces" };
            format!("{} {times} · último {}", host.use_count, relative_time(now - t))
        }
        None => "nunca".into(),
    };
    lines.push(field("Uso", usage));
    lines.push(field(
        "Creada",
        format!("{} ({})", format_date(host.created_at), relative_time(now - host.created_at)),
    ));
    if !app.history.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from("Últimas conexiones".bold().fg(ACCENT)));
        for entry in &app.history {
            let args: Vec<String> = entry.args.iter().map(|a| shell_quote(a)).collect();
            lines.push(Line::from(vec![
                format!("{:<12}", relative_time(now - entry.connected_at)).dim(),
                format!("sshh {}", args.join(" ")).into(),
            ]));
        }
    }
    if let Some(notes) = &d.notes {
        lines.push(Line::default());
        lines.push(Line::from("Notas".bold().fg(ACCENT)));
        lines.extend(Text::from(notes.as_str()).lines);
    }

    let p = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    // Limita el scroll para no dejar el panel vacío.
    let max_scroll = (p.line_count(area.width) as u16).saturating_sub(area.height);
    let scroll = app.detail_scroll.min(max_scroll);
    frame.render_widget(p.scroll((scroll, 0)), area);
    app.detail_scroll = scroll;
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(status) = &app.status {
        let color = if status.error { Color::Red } else { Color::Green };
        frame.render_widget(Line::from(format!(" {}", status.text)).fg(color), area);
        return;
    }
    let keys: &[(&str, &str)] = match (&app.mode, app.focus) {
        (Mode::Search, _) => &[
            ("Enter", "conectar"),
            ("↑↓ Ctrl-j/k", "mover"),
            ("Esc", "terminar búsqueda"),
        ],
        (_, Focus::Detail) => &[("j/k", "scroll"), ("Tab/Esc", "volver a la lista"), ("q", "salir")],
        _ => &[
            ("Enter", "conectar"),
            ("/", "buscar"),
            ("a", "nueva"),
            ("e", "editar"),
            ("dd", "borrar"),
            ("yy", "copiar"),
            ("?", "ayuda"),
            ("q", "salir"),
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

fn draw_confirm(frame: &mut Frame, alias: &str) {
    let lines = vec![
        Line::from(vec!["¿Borrar la conexión ".into(), alias.to_string().bold(), "?".into()]),
        Line::default(),
        Line::from(vec![
            " y ".fg(Color::Red).bold(),
            "borrar   ".dim(),
            " cualquier otra tecla ".fg(ACCENT).bold(),
            "cancelar".dim(),
        ]),
    ];
    let area = centered(frame.area(), 52, 5);
    frame.render_widget(Clear, area);
    let block = block("Confirmar", true).border_style(Style::new().fg(Color::Red));
    frame.render_widget(Paragraph::new(lines).centered().block(block), area);
}

fn draw_help(frame: &mut Frame) {
    const HELP: &[(&str, &str)] = &[
        ("Enter", "conectar"),
        ("j/k ↑/↓", "mover"),
        ("gg / G", "ir al principio / al final"),
        ("Ctrl-d/u", "media página abajo / arriba"),
        ("Ctrl-f/b", "página abajo / arriba"),
        ("Tab", "foco lista ↔ detalle (scroll de notas)"),
        ("", ""),
        ("/", "buscar (fuzzy); #tag filtra por tag"),
        ("Ctrl-j/k", "en la búsqueda, mover"),
        ("Ctrl-w/u", "en la búsqueda, borra palabra / todo"),
        ("Esc", "volver / limpiar búsqueda"),
        ("", ""),
        ("a", "nueva conexión"),
        ("e", "editar"),
        ("t", "editar tags"),
        ("dd", "borrar (pide confirmación)"),
        ("yy", "copiar el comando ssh"),
        ("s", "abrir sftp"),
        ("c", "instalar tu clave pública (ssh-copy-id)"),
        ("o", "cambiar orden (recientes / más usadas / alfabético)"),
        ("R", "recargar"),
        ("q / Ctrl-c", "salir"),
        ("", ""),
        ("Click", "seleccionar · doble click conecta"),
        ("Rueda", "mover / scroll del detalle"),
    ];
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, desc)| Line::from(vec![format!(" {k:<12}").fg(ACCENT).bold(), (*desc).into()]))
        .collect();
    let area = centered(frame.area(), 66, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block("Ayuda", true)), area);
}
