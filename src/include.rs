//! Fichero ssh_config generado (`~/.ssh/config.d/sshh.conf`).
//!
//! sshh nunca modifica `~/.ssh/config`: el usuario añade a mano
//! `Include config.d/sshh.conf` al principio. El fichero se activa al crearlo
//! (`sshh ssh-config sync`) y, mientras exista, se regenera tras cada cambio.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::db::Db;
use crate::model::{Host, config_value};
use crate::ssh_config::{self, GENERATED_FILE_NAME};

/// Línea que hay que añadir a `~/.ssh/config`.
pub const INCLUDE_LINE: &str = "Include config.d/sshh.conf";

fn ssh_dir() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".ssh"))
        .ok_or_else(|| anyhow!("no se pudo determinar el directorio home"))
}

/// `$SSHH_INCLUDE_FILE` o `~/.ssh/config.d/sshh.conf`.
pub fn path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SSHH_INCLUDE_FILE") {
        return Ok(path.into());
    }
    Ok(ssh_dir()?.join("config.d").join(GENERATED_FILE_NAME))
}

pub fn is_enabled() -> bool {
    path().is_ok_and(|p| p.exists())
}

pub fn render(hosts: &[Host]) -> String {
    let mut hosts: Vec<&Host> = hosts.iter().collect();
    hosts.sort_by(|a, b| a.data.alias.cmp(&b.data.alias));
    let mut out = format!(
        "# Generado por sshh: no lo edites, se sobrescribe con cada cambio.\n\
         # Para que ssh lo use, añade al PRINCIPIO de ~/.ssh/config:\n\
         #   {INCLUDE_LINE}\n"
    );
    for host in hosts {
        let d = &host.data;
        out.push('\n');
        let comment: Vec<&str> = [d.name.as_deref(), d.description.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if !comment.is_empty() {
            out.push_str(&format!("# {}\n", comment.join(" — ").replace('\n', " ")));
        }
        out.push_str(&format!("Host {}\n", d.alias));
        for (key, value) in d.ssh_options() {
            out.push_str(&format!("    {key} {}\n", config_value(&key, &value)));
        }
    }
    out
}

/// Escribe el fichero (de forma atómica) y devuelve su ruta.
pub fn write(db: &Db) -> Result<PathBuf> {
    let path = path()?;
    let dir = path.parent().context("ruta sin directorio")?;
    if !dir.exists() {
        fs::create_dir_all(dir).with_context(|| format!("creando {}", dir.display()))?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let content = render(&db.list_hosts()?);
    let tmp = dir.join(format!(".{GENERATED_FILE_NAME}.tmp"));
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("creando {}", tmp.display()))?
        .write_all(content.as_bytes())?;
    fs::rename(&tmp, &path).with_context(|| format!("escribiendo {}", path.display()))?;
    Ok(path)
}

/// Regenera el fichero si está activado. Llamar después de cada cambio.
pub fn refresh(db: &Db) -> Result<()> {
    if is_enabled() {
        write(db)?;
    }
    Ok(())
}

/// Como `refresh`, pero solo avisa por stderr si falla (para la CLI).
pub fn refresh_or_warn(db: &Db) {
    if let Err(e) = refresh(db) {
        eprintln!("sshh: no se pudo actualizar el fichero de ssh_config: {e:#}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeState {
    /// `~/.ssh/config` lo incluye antes de cualquier `Host`/`Match`.
    Ok,
    /// Lo incluye, pero dentro de un bloque `Host`/`Match`: no se aplica bien.
    Misplaced,
    Missing,
}

/// Comprueba si `~/.ssh/config` incluye el fichero generado.
pub fn include_state() -> Result<IncludeState> {
    let ssh_dir = ssh_dir()?;
    let config = ssh_dir.join("config");
    let text = match fs::read_to_string(&config) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(IncludeState::Missing),
        Err(e) => return Err(e).with_context(|| format!("leyendo {}", config.display())),
    };
    Ok(scan_includes(&text, &ssh_dir, &path()?))
}

fn scan_includes(text: &str, ssh_dir: &Path, target: &Path) -> IncludeState {
    let mut in_block = false;
    for line in text.lines().map(str::trim) {
        let Some((key, value)) = crate::ssh_args::parse_option(line) else { continue };
        match key.to_ascii_lowercase().as_str() {
            "host" | "match" => in_block = true,
            "include" if value.split_whitespace().any(|p| includes(p, ssh_dir, target)) => {
                return if in_block { IncludeState::Misplaced } else { IncludeState::Ok };
            }
            _ => {}
        }
    }
    IncludeState::Missing
}

/// Indica si el patrón de un `Include` cubre `target`.
fn includes(pattern: &str, ssh_dir: &Path, target: &Path) -> bool {
    let pattern = match pattern.strip_prefix("~/") {
        Some(rest) => ssh_dir.parent().unwrap_or(ssh_dir).join(rest),
        None => PathBuf::from(pattern),
    };
    let pattern = if pattern.is_absolute() { pattern } else { ssh_dir.join(pattern) };
    let (Some(pat_name), Some(name)) = (
        pattern.file_name().and_then(|n| n.to_str()),
        target.file_name().and_then(|n| n.to_str()),
    ) else {
        return false;
    };
    pattern.parent() == target.parent() && ssh_config::wildcard_match(pat_name, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HostData, SshOption};

    fn host(alias: &str) -> Host {
        Host {
            id: 1,
            data: HostData {
                alias: alias.into(),
                hostname: "10.0.0.5".into(),
                user: Some("deploy".into()),
                port: Some(2222),
                identity_file: Some("~/.ssh/my key".into()),
                extra_options: vec![SshOption { key: "LocalForward".into(), value: "8080 localhost:80".into() }],
                name: Some("Web".into()),
                description: Some("Frontend".into()),
                notes: Some("secreto".into()),
                ..Default::default()
            },
            created_at: 0,
            updated_at: 0,
            last_used: None,
            use_count: 0,
        }
    }

    #[test]
    fn renders_hosts_sorted_without_notes() {
        let out = render(&[host("web"), host("api")]);
        assert!(out.find("Host api").unwrap() < out.find("Host web").unwrap());
        assert!(out.contains(
            "# Web — Frontend\nHost web\n    HostName 10.0.0.5\n    User deploy\n    Port 2222\n    \
             IdentityFile \"~/.ssh/my key\"\n    LocalForward 8080 localhost:80\n"
        ));
        assert!(!out.contains("secreto"));
    }

    #[test]
    fn rendered_config_is_valid_for_ssh() {
        let dir = std::env::temp_dir().join(format!("sshh-inc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config");
        fs::write(&file, render(&[host("web")])).unwrap();
        let out = std::process::Command::new("ssh").arg("-G").arg("-F").arg(&file).arg("web").output();
        fs::remove_dir_all(&dir).unwrap();
        let Ok(out) = out else { return }; // sin ssh instalado no se comprueba
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(text.contains("hostname 10.0.0.5"));
        assert!(text.contains("port 2222"));
        assert!(text.contains("localforward 8080 [localhost]:80"));
    }

    #[test]
    fn detects_include() {
        let ssh = Path::new("/home/u/.ssh");
        let target = ssh.join("config.d/sshh.conf");
        let state = |text: &str| scan_includes(text, ssh, &target);
        assert_eq!(state("Include config.d/sshh.conf\nHost a\n"), IncludeState::Ok);
        assert_eq!(state("Include ~/.ssh/config.d/*\n"), IncludeState::Ok);
        assert_eq!(state("Include /home/u/.ssh/config.d/*.conf\n"), IncludeState::Ok);
        assert_eq!(state("Host a\n  User b\nInclude config.d/sshh.conf\n"), IncludeState::Misplaced);
        assert_eq!(state("Include config.d/otro.conf\n"), IncludeState::Missing);
        assert_eq!(state(""), IncludeState::Missing);
    }
}
