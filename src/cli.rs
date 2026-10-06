//! Subcomandos propios de sshh (`sshh ls`, `sshh add`, ...).

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::connect::shell_quote;
use crate::db::{self, Db, ImportReport, OnConflict};
use crate::include::{self, INCLUDE_LINE, IncludeState};
use crate::model::{HostData, RESERVED_ALIASES, SshOption};
use crate::ssh_args::{Destination, parse_option};
use crate::ssh_config;

#[derive(Parser)]
#[command(
    name = "sshh",
    version,
    about = "Gestor de conexiones SSH",
    after_help = "Cualquier otro argumento se pasa a ssh: `sshh [opciones de ssh] destino [comando]`.\n\
                  Sin argumentos abre el TUI.\n\
                  Para conectar a un host que se llame como un subcomando usa `sshh -- <host>`."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lista las conexiones guardadas
    Ls,
    /// Guarda una conexión nueva
    Add(AddArgs),
    /// Elimina una conexión
    Rm { alias: String },
    /// Muestra las últimas conexiones (de todas o de un alias)
    History {
        alias: Option<String>,
        /// Número máximo de entradas
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Exporta las conexiones a JSON (sin historial)
    Export {
        /// Fichero de salida (por defecto, la salida estándar)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Importa conexiones desde un JSON generado con `sshh export` (`-` = stdin)
    Import {
        file: PathBuf,
        #[command(flatten)]
        opts: ImportOpts,
    },
    /// Importa los bloques Host de ~/.ssh/config (y sus Include)
    ImportSshConfig {
        /// Fichero ssh_config (por defecto ~/.ssh/config)
        file: Option<PathBuf>,
        /// Tags que añadir a las conexiones importadas, separados por comas
        #[arg(short, long = "tag", value_delimiter = ',')]
        tags: Vec<String>,
        #[command(flatten)]
        opts: ImportOpts,
    },
    /// Gestiona ~/.ssh/config.d/sshh.conf, para que ssh, scp, rsync o git conozcan tus alias
    SshConfig {
        #[command(subcommand)]
        action: Option<SshConfigAction>,
    },
}

#[derive(Subcommand)]
enum SshConfigAction {
    /// Muestra si está activado y si ~/.ssh/config lo incluye (por defecto)
    Status,
    /// Genera el fichero y lo mantiene actualizado a partir de ahora
    Sync,
    /// Imprime el contenido que se generaría
    Print,
    /// Borra el fichero y deja de actualizarlo
    Disable,
}

#[derive(Args)]
struct AddArgs {
    /// Alias con el que se usará la conexión (`sshh <alias>`)
    alias: String,
    /// Destino: [user@]host o ssh://[user@]host[:port]
    destination: String,
    #[arg(short, long)]
    port: Option<u16>,
    #[arg(short, long)]
    identity: Option<String>,
    /// ProxyJump
    #[arg(short = 'J', long)]
    jump: Option<String>,
    /// Opción extra de ssh_config (Key=Value); se puede repetir
    #[arg(short = 'o', long = "option", value_parser = parse_option_arg)]
    options: Vec<SshOption>,
    #[arg(short, long)]
    name: Option<String>,
    #[arg(short, long)]
    description: Option<String>,
    #[arg(long)]
    notes: Option<String>,
    /// Tags separados por comas
    #[arg(short, long = "tag", value_delimiter = ',')]
    tags: Vec<String>,
}

#[derive(Args)]
struct ImportOpts {
    /// Qué hacer si ya existe una conexión con el mismo alias y otros datos
    #[arg(long, value_enum, default_value_t)]
    on_conflict: OnConflict,
    /// Muestra qué se importaría sin guardar nada
    #[arg(short = 'n', long)]
    dry_run: bool,
}

/// Formato de `sshh export`.
#[derive(Serialize, Deserialize)]
struct ExportFile {
    version: u32,
    exported_at: String,
    hosts: Vec<HostData>,
}

/// `sshh import` acepta el fichero de export o directamente una lista.
#[derive(Deserialize)]
#[serde(untagged)]
enum ImportFile {
    Export { hosts: Vec<HostData> },
    List(Vec<HostData>),
}

fn parse_option_arg(s: &str) -> Result<SshOption, String> {
    parse_option(s)
        .map(|(key, value)| SshOption { key, value })
        .ok_or_else(|| format!("se esperaba Key=Value, no «{s}»"))
}

/// Indica si los argumentos son para sshh y no para ssh.
pub fn is_own_command(args: &[String]) -> bool {
    args.first().is_some_and(|a| {
        RESERVED_ALIASES.contains(&a.as_str()) || matches!(a.as_str(), "--help" | "--version")
    })
}

pub fn run(args: Vec<String>) -> Result<()> {
    let cli = Cli::parse_from(std::iter::once("sshh".to_string()).chain(args));
    match cli.command {
        Command::Ls => list(),
        Command::Add(args) => add(args),
        Command::Rm { alias } => {
            let db = Db::open_default()?;
            if !db.delete_by_alias(&alias)? {
                bail!("no existe ninguna conexión «{alias}»");
            }
            include::refresh_or_warn(&db);
            println!("Conexión «{alias}» eliminada");
            Ok(())
        }
        Command::History { alias, limit } => history(alias.as_deref(), limit),
        Command::Export { output } => export(output),
        Command::Import { file, opts } => import(file, opts),
        Command::ImportSshConfig { file, tags, opts } => import_ssh_config(file, tags, opts),
        Command::SshConfig { action } => ssh_config_cmd(action.unwrap_or(SshConfigAction::Status)),
    }
}

fn ssh_config_cmd(action: SshConfigAction) -> Result<()> {
    let path = include::path()?;
    match action {
        SshConfigAction::Print => print!("{}", include::render(&Db::open_default()?.list_hosts()?)),
        SshConfigAction::Sync => {
            let db = Db::open_default()?;
            include::write(&db)?;
            println!("{} conexiones escritas en {}", db.list_hosts()?.len(), path.display());
            println!("Se actualizará automáticamente con cada cambio.");
            print_include_state()?;
        }
        SshConfigAction::Disable => {
            if path.exists() {
                fs::remove_file(&path).with_context(|| format!("borrando {}", path.display()))?;
                println!("Borrado {}. Ya no se actualizará.", path.display());
                println!("La línea `{INCLUDE_LINE}` de ~/.ssh/config puede quedarse: ssh ignora el fichero si no existe.");
            } else {
                println!("No estaba activado.");
            }
        }
        SshConfigAction::Status => {
            if path.exists() {
                println!("Activado: {}", path.display());
            } else {
                println!("Desactivado. Actívalo con `sshh ssh-config sync`.");
            }
            print_include_state()?;
        }
    }
    Ok(())
}

fn print_include_state() -> Result<()> {
    match include::include_state()? {
        IncludeState::Ok => println!("~/.ssh/config lo incluye correctamente."),
        IncludeState::Missing => {
            println!("~/.ssh/config todavía no lo incluye. Añade esta línea al PRINCIPIO del fichero:");
            println!("\n    {INCLUDE_LINE}\n");
        }
        IncludeState::Misplaced => {
            println!("~/.ssh/config lo incluye, pero después de un bloque Host/Match, así que solo se");
            println!("aplica dentro de ese bloque. Mueve la línea al PRINCIPIO del fichero:");
            println!("\n    {INCLUDE_LINE}\n");
        }
    }
    Ok(())
}

fn add(args: AddArgs) -> Result<()> {
    let Some(dest) = Destination::parse(&args.destination) else {
        bail!("destino inválido «{}»", args.destination);
    };
    let data = HostData {
        alias: args.alias,
        hostname: dest.host,
        user: dest.user,
        port: args.port.or(dest.port),
        identity_file: args.identity,
        proxy_jump: args.jump,
        extra_options: args.options,
        name: args.name,
        description: args.description,
        notes: args.notes,
        tags: args.tags,
    };
    let mut db = Db::open_default()?;
    db.insert_host(&data)?;
    include::refresh_or_warn(&db);
    println!("Conexión «{}» guardada", data.alias);
    Ok(())
}

pub fn list() -> Result<()> {
    let hosts = Db::open_default()?.list_hosts()?;
    if hosts.is_empty() {
        println!("No hay conexiones guardadas.");
        println!("Añade una con `sshh add <alias> <destino>` o importa tu ~/.ssh/config con `sshh import-ssh-config`.");
        return Ok(());
    }
    let now = db::now();
    let rows = hosts.iter().map(|h| {
        let d = &h.data;
        vec![
            d.alias.clone(),
            d.target(),
            d.name.clone().unwrap_or_default(),
            d.tags.join(","),
            h.last_used.map(|t| relative_time(now - t)).unwrap_or_else(|| "nunca".into()),
        ]
    });
    print_table(&["ALIAS", "DESTINO", "NOMBRE", "TAGS", "ÚLTIMO USO"], rows.collect());
    Ok(())
}

fn history(alias: Option<&str>, limit: usize) -> Result<()> {
    let db = Db::open_default()?;
    let host_id = match alias {
        Some(alias) => match db.find_by_alias(alias)? {
            Some(host) => Some(host.id),
            None => bail!("no existe ninguna conexión «{alias}»"),
        },
        None => None,
    };
    let entries = db.history(host_id, limit)?;
    if entries.is_empty() {
        println!("Todavía no hay conexiones en el historial.");
        return Ok(());
    }
    let rows = entries.iter().map(|e| {
        let args: Vec<String> = e.args.iter().map(|a| shell_quote(a)).collect();
        vec![format_date(e.connected_at), e.alias.clone(), format!("sshh {}", args.join(" "))]
    });
    print_table(&["FECHA", "ALIAS", "COMANDO"], rows.collect());
    Ok(())
}

fn export(output: Option<PathBuf>) -> Result<()> {
    let mut hosts: Vec<HostData> = Db::open_default()?.list_hosts()?.into_iter().map(|h| h.data).collect();
    hosts.sort_by(|a, b| a.alias.cmp(&b.alias));
    let file = ExportFile {
        version: 1,
        exported_at: chrono::Local::now().to_rfc3339(),
        hosts,
    };
    let json = serde_json::to_string_pretty(&file)? + "\n";
    match output {
        None => std::io::stdout().write_all(json.as_bytes())?,
        Some(path) => {
            // Las notas pueden contener información sensible.
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)
                .with_context(|| format!("creando {}", path.display()))?
                .write_all(json.as_bytes())?;
            eprintln!("{} conexiones exportadas a {}", file.hosts.len(), path.display());
        }
    }
    Ok(())
}

fn import(file: PathBuf, opts: ImportOpts) -> Result<()> {
    let text = if file.as_os_str() == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text
    } else {
        fs::read_to_string(&file).with_context(|| format!("leyendo {}", file.display()))?
    };
    // Primero se valida la sintaxis (para que el error indique línea y columna).
    let value: serde_json::Value = serde_json::from_str(&text).context("JSON inválido")?;
    let hosts = match serde_json::from_value(value) {
        Ok(ImportFile::Export { hosts } | ImportFile::List(hosts)) => hosts,
        Err(_) => bail!(
            "el JSON no tiene el formato de `sshh export`: se espera {{\"hosts\": [...]}} \
             o una lista de conexiones con al menos \"alias\" y \"hostname\""
        ),
    };
    let mut db = Db::open_default()?;
    let report = db.import_hosts(&hosts, opts.on_conflict, opts.dry_run)?;
    if !opts.dry_run {
        include::refresh_or_warn(&db);
    }
    print_report(&report, opts.dry_run);
    Ok(())
}

fn import_ssh_config(file: Option<PathBuf>, tags: Vec<String>, opts: ImportOpts) -> Result<()> {
    let path = match file.or_else(ssh_config::default_path) {
        Some(path) => path,
        None => bail!("no se pudo determinar la ruta de ~/.ssh/config"),
    };
    let mut parsed = ssh_config::parse_file(&path)?;
    for host in &mut parsed.hosts {
        host.tags.extend(tags.iter().cloned());
    }
    if parsed.hosts.is_empty() {
        println!("No hay bloques Host importables en {}", path.display());
    } else {
        let mut db = Db::open_default()?;
        let report = db.import_hosts(&parsed.hosts, opts.on_conflict, opts.dry_run)?;
        if !opts.dry_run {
            include::refresh_or_warn(&db);
        }
        print_report(&report, opts.dry_run);
    }
    if !parsed.ignored.is_empty() {
        println!("\nNo importados (ssh los sigue aplicando igualmente):");
        for ignored in &parsed.ignored {
            println!("  · {} ({})", ignored.pattern, ignored.reason);
        }
    }
    Ok(())
}

fn print_report(r: &ImportReport, dry_run: bool) {
    let counts = [
        (r.added.len(), "nueva", "nuevas"),
        (r.updated.len(), "actualizada", "actualizadas"),
        (r.renamed.len(), "renombrada", "renombradas"),
        (r.unchanged.len(), "sin cambios", "sin cambios"),
        (r.skipped.len(), "omitida", "omitidas"),
    ];
    let summary: Vec<String> = counts
        .iter()
        .filter(|(n, ..)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if *n == 1 { one } else { many }))
        .collect();
    if summary.is_empty() {
        println!("No había nada que importar.");
        return;
    }
    println!("{}", summary.join(", "));
    for alias in &r.added {
        println!("  + {alias}");
    }
    for alias in &r.updated {
        println!("  ~ {alias}");
    }
    for (from, to) in &r.renamed {
        println!("  + {to} (renombrada desde «{from}»)");
    }
    for (alias, reason) in &r.skipped {
        println!("  ! {alias}: {reason}");
    }
    if r.skipped.iter().any(|(_, reason)| reason.starts_with("ya existe")) {
        println!("Usa --on-conflict overwrite|rename para importar las que ya existen.");
    }
    if dry_run {
        println!("(simulación: no se ha guardado nada)");
    }
}

fn print_table(header: &[&str], rows: Vec<Vec<String>>) {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let header = header.iter().map(|h| h.to_string()).collect();
    for row in std::iter::once(header).chain(rows) {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
}

pub fn relative_time(secs: i64) -> String {
    match secs {
        ..60 => "ahora".into(),
        60..3600 => format!("hace {} min", secs / 60),
        3600..86400 => format!("hace {} h", secs / 3600),
        _ => format!("hace {} d", secs / 86400),
    }
}

/// Fecha local `AAAA-MM-DD HH:MM`.
pub fn format_date(timestamp: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
