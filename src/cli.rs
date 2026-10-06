//! sshh's own subcommands (`sshh ls`, `sshh add`, ...).

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
    about = "SSH connection manager",
    after_help = "Any other arguments are passed to ssh: `sshh [ssh options] destination [command]`.\n\
                  Without arguments it opens the TUI.\n\
                  To connect to a host named like a subcommand use `sshh -- <host>`."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List saved connections
    Ls,
    /// Save a new connection
    Add(AddArgs),
    /// Delete a connection
    Rm { alias: String },
    /// Show the latest connections (of all of them or of one alias)
    History {
        alias: Option<String>,
        /// Maximum number of entries
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Export connections to JSON (without history)
    Export {
        /// Output file (standard output by default)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Import connections from a JSON file created by `sshh export` (`-` = stdin)
    Import {
        file: PathBuf,
        #[command(flatten)]
        opts: ImportOpts,
    },
    /// Import the Host blocks of ~/.ssh/config (and its Includes)
    ImportSshConfig {
        /// ssh_config file (~/.ssh/config by default)
        file: Option<PathBuf>,
        /// Tags to add to the imported connections, comma separated
        #[arg(short, long = "tag", value_delimiter = ',')]
        tags: Vec<String>,
        #[command(flatten)]
        opts: ImportOpts,
    },
    /// Manage ~/.ssh/config.d/sshh.conf, so ssh, scp, rsync or git know your aliases
    SshConfig {
        #[command(subcommand)]
        action: Option<SshConfigAction>,
    },
}

#[derive(Subcommand)]
enum SshConfigAction {
    /// Show whether it is enabled and whether ~/.ssh/config includes it (default)
    Status,
    /// Generate the file and keep it up to date from now on
    Sync,
    /// Print the content that would be generated
    Print,
    /// Delete the file and stop updating it
    Disable,
}

#[derive(Args)]
struct AddArgs {
    /// Alias used for the connection (`sshh <alias>`)
    alias: String,
    /// Destination: [user@]host or ssh://[user@]host[:port]
    destination: String,
    #[arg(short, long)]
    port: Option<u16>,
    #[arg(short, long)]
    identity: Option<String>,
    /// ProxyJump
    #[arg(short = 'J', long)]
    jump: Option<String>,
    /// Extra ssh_config option (Key=Value); can be repeated
    #[arg(short = 'o', long = "option", value_parser = parse_option_arg)]
    options: Vec<SshOption>,
    #[arg(short, long)]
    name: Option<String>,
    #[arg(short, long)]
    description: Option<String>,
    #[arg(long)]
    notes: Option<String>,
    /// Comma separated tags
    #[arg(short, long = "tag", value_delimiter = ',')]
    tags: Vec<String>,
}

#[derive(Args)]
struct ImportOpts {
    /// What to do when a connection with the same alias and different data exists
    #[arg(long, value_enum, default_value_t)]
    on_conflict: OnConflict,
    /// Show what would be imported without saving anything
    #[arg(short = 'n', long)]
    dry_run: bool,
}

/// `sshh export` format.
#[derive(Serialize, Deserialize)]
struct ExportFile {
    version: u32,
    exported_at: String,
    hosts: Vec<HostData>,
}

/// `sshh import` accepts the export file or a plain list.
#[derive(Deserialize)]
#[serde(untagged)]
enum ImportFile {
    Export { hosts: Vec<HostData> },
    List(Vec<HostData>),
}

fn parse_option_arg(s: &str) -> Result<SshOption, String> {
    parse_option(s)
        .map(|(key, value)| SshOption { key, value })
        .ok_or_else(|| format!("expected Key=Value, got '{s}'"))
}

/// Whether the arguments are meant for sshh rather than ssh.
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
                bail!("no connection named '{alias}'");
            }
            include::refresh_or_warn(&db);
            println!("Connection '{alias}' deleted");
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
            println!("{} connections written to {}", db.list_hosts()?.len(), path.display());
            println!("It will be updated automatically on every change.");
            print_include_state()?;
        }
        SshConfigAction::Disable => {
            if path.exists() {
                fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))?;
                println!("Deleted {}. It will no longer be updated.", path.display());
                println!("The `{INCLUDE_LINE}` line in ~/.ssh/config can stay: ssh ignores the file if it doesn't exist.");
            } else {
                println!("It was not enabled.");
            }
        }
        SshConfigAction::Status => {
            if path.exists() {
                println!("Enabled: {}", path.display());
            } else {
                println!("Disabled. Enable it with `sshh ssh-config sync`.");
            }
            print_include_state()?;
        }
    }
    Ok(())
}

fn print_include_state() -> Result<()> {
    match include::include_state()? {
        IncludeState::Ok => println!("~/.ssh/config includes it correctly."),
        IncludeState::Missing => {
            println!("~/.ssh/config doesn't include it yet. Add this line at the TOP of the file:");
            println!("\n    {INCLUDE_LINE}\n");
        }
        IncludeState::Misplaced => {
            println!("~/.ssh/config includes it, but after a Host/Match block, so it only applies");
            println!("inside that block. Move the line to the TOP of the file:");
            println!("\n    {INCLUDE_LINE}\n");
        }
    }
    Ok(())
}

fn add(args: AddArgs) -> Result<()> {
    let Some(dest) = Destination::parse(&args.destination) else {
        bail!("invalid destination '{}'", args.destination);
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
    println!("Connection '{}' saved", data.alias);
    Ok(())
}

pub fn list() -> Result<()> {
    let hosts = Db::open_default()?.list_hosts()?;
    if hosts.is_empty() {
        println!("No saved connections.");
        println!("Add one with `sshh add <alias> <destination>` or import your ~/.ssh/config with `sshh import-ssh-config`.");
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
            h.last_used.map(|t| relative_time(now - t)).unwrap_or_else(|| "never".into()),
        ]
    });
    print_table(&["ALIAS", "TARGET", "NAME", "TAGS", "LAST USED"], rows.collect());
    Ok(())
}

fn history(alias: Option<&str>, limit: usize) -> Result<()> {
    let db = Db::open_default()?;
    let host_id = match alias {
        Some(alias) => match db.find_by_alias(alias)? {
            Some(host) => Some(host.id),
            None => bail!("no connection named '{alias}'"),
        },
        None => None,
    };
    let entries = db.history(host_id, limit)?;
    if entries.is_empty() {
        println!("No connections in the history yet.");
        return Ok(());
    }
    let rows = entries.iter().map(|e| {
        let args: Vec<String> = e.args.iter().map(|a| shell_quote(a)).collect();
        vec![format_date(e.connected_at), e.alias.clone(), format!("sshh {}", args.join(" "))]
    });
    print_table(&["DATE", "ALIAS", "COMMAND"], rows.collect());
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
            // Notes may contain sensitive information.
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)
                .with_context(|| format!("creating {}", path.display()))?
                .write_all(json.as_bytes())?;
            eprintln!("{} connections exported to {}", file.hosts.len(), path.display());
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
        fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?
    };
    // Check the syntax first (so the error reports line and column).
    let value: serde_json::Value = serde_json::from_str(&text).context("invalid JSON")?;
    let hosts = match serde_json::from_value(value) {
        Ok(ImportFile::Export { hosts } | ImportFile::List(hosts)) => hosts,
        Err(_) => bail!(
            "the JSON is not in `sshh export` format: expected {{\"hosts\": [...]}} \
             or a list of connections with at least \"alias\" and \"hostname\""
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
        None => bail!("could not determine the path of ~/.ssh/config"),
    };
    let mut parsed = ssh_config::parse_file(&path)?;
    for host in &mut parsed.hosts {
        host.tags.extend(tags.iter().cloned());
    }
    if parsed.hosts.is_empty() {
        println!("No importable Host blocks in {}", path.display());
    } else {
        let mut db = Db::open_default()?;
        let report = db.import_hosts(&parsed.hosts, opts.on_conflict, opts.dry_run)?;
        if !opts.dry_run {
            include::refresh_or_warn(&db);
        }
        print_report(&report, opts.dry_run);
    }
    if !parsed.ignored.is_empty() {
        println!("\nNot imported (ssh still applies them anyway):");
        for ignored in &parsed.ignored {
            println!("  · {} ({})", ignored.pattern, ignored.reason);
        }
    }
    Ok(())
}

fn print_report(r: &ImportReport, dry_run: bool) {
    let counts = [
        (r.added.len(), "new"),
        (r.updated.len(), "updated"),
        (r.renamed.len(), "renamed"),
        (r.unchanged.len(), "unchanged"),
        (r.skipped.len(), "skipped"),
    ];
    let summary: Vec<String> = counts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{n} {label}"))
        .collect();
    if summary.is_empty() {
        println!("Nothing to import.");
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
        println!("  + {to} (renamed from '{from}')");
    }
    for (alias, reason) in &r.skipped {
        println!("  ! {alias}: {reason}");
    }
    if r.skipped.iter().any(|(_, reason)| reason.starts_with("already exists")) {
        println!("Use --on-conflict overwrite|rename to import the existing ones.");
    }
    if dry_run {
        println!("(dry run: nothing was saved)");
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
        ..60 => "now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}

/// Local date `YYYY-MM-DD HH:MM`.
pub fn format_date(timestamp: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
