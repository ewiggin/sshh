//! Wrapper mode: `sshh [ssh arguments]`.
//!
//! Always ends up running the system `ssh` with `exec`. sshh must never get in
//! the way of a connection: if something fails (parser, database) it warns and
//! passes the arguments through unchanged.

use std::convert::Infallible;
use std::ffi::OsStr;
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, fs};

use anyhow::{Context, Result, anyhow};

use crate::db::Db;
use crate::model::{Host, HostData, config_value, validate_alias};
use crate::ssh_args::{self, Destination, SshInvocation};
use crate::tui::{self, WizardOutcome};

/// How the destination was matched against the database.
enum Resolution {
    /// The destination is the alias of a saved connection.
    Alias(Host),
    /// The destination matches `user@hostname:port` of a saved connection.
    Target(Host),
    Unknown(Target),
}

/// Where ssh would really connect (per `ssh -G`, which applies ssh_config).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    hostname: String,
    user: String,
    port: u16,
    proxy_jump: Option<String>,
}

pub fn run(args: Vec<String>) -> Result<Infallible> {
    let inv = match ssh_args::parse(&args) {
        Ok(inv) => inv,
        Err(e) => {
            debug(format_args!("could not parse the arguments ({e}); passing them through"));
            return exec_ssh(&args);
        }
    };
    let Some((dest_index, dest)) = &inv.destination else {
        debug(format_args!("no destination; passing the arguments through"));
        return exec_ssh(&args);
    };
    let final_args = match prepare(&args, &inv, *dest_index, dest) {
        Ok(Some(final_args)) => final_args,
        Ok(None) => std::process::exit(130),
        Err(e) => {
            eprintln!("sshh: {e:#}; running ssh directly");
            args
        }
    };
    exec_ssh(&final_args)
}

/// Resolves the destination, records the connection and returns the arguments
/// for ssh, or `None` if the user cancelled in the wizard.
fn prepare(
    args: &[String],
    inv: &SshInvocation,
    dest_index: usize,
    dest: &Destination,
) -> Result<Option<Vec<String>>> {
    let mut db = Db::open_default()?;
    let (host_id, final_args) = match resolve(&db, args, inv, dest)? {
        Resolution::Alias(host) => {
            debug(format_args!("'{}' is the alias of a saved connection", dest.host));
            (host.id, alias_args(args, dest_index, inv, &host))
        }
        Resolution::Target(host) => {
            debug(format_args!("'{dest}' matches saved connection '{}'", host.data.alias));
            (host.id, args.to_vec())
        }
        Resolution::Unknown(target) => {
            debug(format_args!("'{dest}' is not saved ({target:?})"));
            if inv.info_only || !wizard_enabled() {
                return Ok(Some(args.to_vec()));
            }
            let prefill = prefill(dest, inv, &target);
            match tui::wizard(&mut db, &prefill, &dest.to_string())? {
                WizardOutcome::Saved(id) => {
                    crate::include::refresh_or_warn(&db);
                    (id, args.to_vec())
                }
                WizardOutcome::Skipped => return Ok(Some(args.to_vec())),
                WizardOutcome::Aborted => return Ok(None),
            }
        }
    };
    if !inv.info_only
        && let Err(e) = db.record_connection(host_id, args)
    {
        eprintln!("sshh: could not save history: {e:#}");
    }
    Ok(Some(final_args))
}

fn resolve(db: &Db, args: &[String], inv: &SshInvocation, dest: &Destination) -> Result<Resolution> {
    if let Some(host) = db.find_by_alias(&dest.host)? {
        return Ok(Resolution::Alias(host));
    }
    let target = resolve_target(args, inv, dest);
    let found = db.find_by_target(&target.hostname, &target.user, target.port, &local_user())?;
    Ok(match found {
        Some(host) => Resolution::Target(host),
        None => Resolution::Unknown(target),
    })
}

/// The wizard only shows up in interactive sessions and can be disabled with
/// `SSHH_NO_WIZARD=1`.
fn wizard_enabled() -> bool {
    let off = env::var_os("SSHH_NO_WIZARD").is_some_and(|v| !v.is_empty() && v != "0");
    !off && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
}

fn local_user() -> String {
    env::var("USER").or_else(|_| env::var("LOGNAME")).unwrap_or_default()
}

/// Asks `ssh -G` for the effective destination; if that fails, uses what is in
/// the arguments.
fn resolve_target(args: &[String], inv: &SshInvocation, dest: &Destination) -> Target {
    let from_ssh = find_ssh().ok().and_then(|bin| {
        let out = Command::new(bin)
            .arg("-G")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status.success().then(|| parse_ssh_g(&String::from_utf8_lossy(&out.stdout)))?
    });
    from_ssh.unwrap_or_else(|| Target {
        hostname: dest.host.clone(),
        user: inv.effective_user().map_or_else(local_user, String::from),
        port: inv.effective_port().unwrap_or(22),
        proxy_jump: inv.jump.clone(),
    })
}

fn parse_ssh_g(output: &str) -> Option<Target> {
    let value = |key: &str| {
        output
            .lines()
            .find_map(|l| l.split_once(' ').filter(|(k, _)| *k == key).map(|(_, v)| v.trim()))
            .filter(|v| !v.is_empty() && *v != "none")
            .map(String::from)
    };
    Some(Target {
        hostname: value("hostname")?,
        user: value("user")?,
        port: value("port")?.parse().ok()?,
        proxy_jump: value("proxyjump"),
    })
}

/// Initial wizard data from what was typed and from ssh -G.
fn prefill(dest: &Destination, inv: &SshInvocation, target: &Target) -> HostData {
    let explicit_user = inv.effective_user().is_some() || target.user != local_user();
    HostData {
        alias: suggest_alias(&dest.host),
        hostname: target.hostname.clone(),
        user: explicit_user.then(|| target.user.clone()),
        port: (target.port != 22).then_some(target.port),
        identity_file: inv.identities.first().cloned(),
        proxy_jump: target.proxy_jump.clone(),
        ..Default::default()
    }
}

/// `web.example.com` → `web`; IPs and ssh_config aliases stay as they are.
fn suggest_alias(host: &str) -> String {
    let alias = if host.parse::<std::net::IpAddr>().is_ok() {
        host
    } else {
        host.split('.').next().unwrap_or(host)
    };
    if validate_alias(alias).is_ok() { alias.to_string() } else { String::new() }
}

/// ssh binary and arguments to connect to a saved connection from the TUI
/// (embedded session or full screen). Records the connection in the history.
pub fn session_command(db: &Db, host: &Host) -> Result<(PathBuf, Vec<String>)> {
    let args = tool_args(host);
    if let Err(e) = db.record_connection(host.id, std::slice::from_ref(&host.data.alias)) {
        debug(format_args!("could not save history: {e:#}"));
    }
    Ok((find_ssh()?, args))
}

/// Arguments for another OpenSSH tool that accepts `-o` (sftp, ssh-copy-id…):
/// the connection options followed by the alias.
pub fn tool_args(host: &Host) -> Vec<String> {
    alias_args(std::slice::from_ref(&host.data.alias), 0, &SshInvocation::default(), host)
}

/// ssh command equivalent to a connection, to copy it and use it outside sshh.
pub fn ssh_command(d: &HostData) -> String {
    let mut parts = vec!["ssh".to_string()];
    let flags = [
        ("-p", d.port.map(|p| p.to_string())),
        ("-i", d.identity_file.clone()),
        ("-J", d.proxy_jump.clone()),
    ];
    for (flag, value) in flags {
        if let Some(value) = value {
            parts.extend([flag.to_string(), value]);
        }
    }
    for opt in &d.extra_options {
        parts.extend(["-o".to_string(), format!("{}={}", opt.key, opt.value)]);
    }
    parts.push(match &d.user {
        Some(user) => format!("{user}@{}", d.hostname),
        None => d.hostname.clone(),
    });
    parts.iter().map(|p| shell_quote(p)).collect::<Vec<_>>().join(" ")
}

/// Inserts a `-o Key=Value` before the destination for every connection option
/// the user hasn't already set. The destination stays the alias so that
/// `Host alias` blocks in ssh_config still apply.
fn alias_args(args: &[String], dest_index: usize, inv: &SshInvocation, host: &Host) -> Vec<String> {
    let at = if dest_index > 0 && args[dest_index - 1] == "--" {
        dest_index - 1
    } else {
        dest_index
    };
    let mut out = args[..at].to_vec();
    for (key, value) in host.data.ssh_options() {
        if !inv.sets(&key) {
            out.push("-o".into());
            out.push(format!("{key}={}", config_value(&key, &value)));
        }
    }
    out.extend_from_slice(&args[at..]);
    out
}

/// Replaces the current process with `ssh`.
pub fn exec_ssh<S: AsRef<OsStr>>(args: &[S]) -> Result<Infallible> {
    let bin = find_ssh()?;
    if debug_enabled() {
        let cmd: Vec<String> = std::iter::once(bin.as_os_str())
            .chain(args.iter().map(AsRef::as_ref))
            .map(|a| shell_quote(&a.to_string_lossy()))
            .collect();
        debug(format_args!("exec {}", cmd.join(" ")));
    }
    let err = Command::new(&bin).args(args).exec();
    Err(anyhow!(err).context(format!("could not run {}", bin.display())))
}

/// `SSHH_DEBUG=1` prints to stderr how the destination is resolved and the final command.
fn debug_enabled() -> bool {
    env::var_os("SSHH_DEBUG").is_some_and(|v| !v.is_empty() && v != "0")
}

fn debug(msg: std::fmt::Arguments) {
    if debug_enabled() {
        eprintln!("sshh[debug]: {msg}");
    }
}

/// Renders an argument the way it would be typed in a shell (display only).
pub fn shell_quote(arg: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./_-~".contains(c);
    if !arg.is_empty() && arg.chars().all(safe) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Finds the ssh binary: `$SSHH_SSH` or the first `ssh` in PATH that isn't this
/// very executable (in case sshh is installed under the name `ssh`).
fn find_ssh() -> Result<PathBuf> {
    if let Some(bin) = env::var_os("SSHH_SSH") {
        return Ok(bin.into());
    }
    let me = env::current_exe().and_then(fs::canonicalize).ok();
    let path = env::var_os("PATH").context("PATH is not set")?;
    env::split_paths(&path)
        .map(|dir| dir.join("ssh"))
        .find(|c| c.is_file() && fs::canonicalize(c).ok() != me)
        .ok_or_else(|| anyhow!("`ssh` not found in PATH"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HostData;

    fn host() -> Host {
        Host {
            id: 1,
            data: HostData {
                alias: "web".into(),
                hostname: "10.0.0.5".into(),
                user: Some("deploy".into()),
                port: Some(2222),
                identity_file: Some("~/.ssh/my key".into()),
                ..Default::default()
            },
            created_at: 0,
            updated_at: 0,
            last_used: None,
            use_count: 0,
        }
    }

    fn build(cmd: &str) -> String {
        let args: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        let inv = ssh_args::parse(&args).unwrap();
        let (i, _) = inv.destination.clone().unwrap();
        alias_args(&args, i, &inv, &host()).join(" ")
    }

    #[test]
    fn injects_host_options_before_destination() {
        assert_eq!(
            build("-v web uptime"),
            "-v -o HostName=10.0.0.5 -o User=deploy -o Port=2222 \
             -o IdentityFile=\"~/.ssh/my key\" web uptime"
        );
    }

    #[test]
    fn user_flags_win() {
        assert_eq!(
            build("-p 22 -i k root@web"),
            "-p 22 -i k -o HostName=10.0.0.5 root@web"
        );
    }

    #[test]
    fn parses_ssh_g_output() {
        let out = "user deploy\nhostname 10.0.0.5\nport 2222\nproxyjump none\n";
        let t = parse_ssh_g(out).unwrap();
        assert_eq!((t.hostname.as_str(), t.user.as_str(), t.port), ("10.0.0.5", "deploy", 2222));
        assert_eq!(t.proxy_jump, None);
        assert!(parse_ssh_g("garbage").is_none());
    }

    #[test]
    fn alias_suggestions() {
        assert_eq!(suggest_alias("web.example.com"), "web");
        assert_eq!(suggest_alias("10.0.0.5"), "10.0.0.5");
        assert_eq!(suggest_alias("::1"), "::1");
        assert_eq!(suggest_alias("ls"), "");
    }

    #[test]
    fn copyable_command() {
        let d = &host().data;
        assert_eq!(ssh_command(d), "ssh -p 2222 -i '~/.ssh/my key' deploy@10.0.0.5");
        let plain = HostData { hostname: "h".into(), ..Default::default() };
        assert_eq!(ssh_command(&plain), "ssh h");
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(shell_quote("-p"), "-p");
        assert_eq!(shell_quote("IdentityFile=\"~/a b\""), "'IdentityFile=\"~/a b\"'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn options_go_before_double_dash() {
        assert_eq!(
            build("-l x -p 1 -i k -- web"),
            "-l x -p 1 -i k -o HostName=10.0.0.5 -- web"
        );
    }
}
