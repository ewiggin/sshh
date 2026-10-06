//! Lectura de ficheros ssh_config para importar sus `Host`.
//!
//! Solo se importan los bloques `Host` con nombres concretos (sin comodines ni
//! negaciones). Los bloques `Match`, los `Host *` y las opciones globales se
//! ignoran porque ssh ya los aplica igualmente a cualquier conexión.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::{HostData, SshOption};
use crate::ssh_args::parse_option;

/// Nombre del fichero que generará sshh (fase 5); nunca se importa.
pub const GENERATED_FILE_NAME: &str = "sshh.conf";

const MAX_INCLUDE_DEPTH: usize = 16;

/// Bloque `Host` descartado y por qué.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ignored {
    pub pattern: String,
    pub reason: &'static str,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub hosts: Vec<HostData>,
    pub ignored: Vec<Ignored>,
}

pub fn default_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().join(".ssh").join("config"))
}

pub fn parse_file(path: &Path) -> Result<Parsed> {
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut parser = Parser { base, parsed: Parsed::default(), block: None };
    parser.file(path, 0)?;
    parser.finish_block();
    Ok(parser.parsed)
}

#[cfg(test)]
fn parse_str(text: &str, base: &Path) -> Parsed {
    let mut parser = Parser { base: base.to_path_buf(), parsed: Parsed::default(), block: None };
    parser.text(text, 0).unwrap();
    parser.finish_block();
    parser.parsed
}

struct Block {
    aliases: Vec<String>,
    data: HostData,
}

struct Parser {
    /// Directorio contra el que se resuelven los `Include` relativos.
    base: PathBuf,
    parsed: Parsed,
    block: Option<Block>,
}

impl Parser {
    fn file(&mut self, path: &Path, depth: usize) -> Result<()> {
        let text = fs::read_to_string(path).with_context(|| format!("leyendo {}", path.display()))?;
        self.text(&text, depth)
    }

    fn text(&mut self, text: &str, depth: usize) -> Result<()> {
        // Comentarios justo encima de un `Host`: pasan a ser sus notas.
        let mut comments: Vec<&str> = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.is_empty() {
                comments.clear();
                continue;
            }
            if let Some(comment) = line.strip_prefix('#') {
                comments.push(comment.trim());
                continue;
            }
            let Some((key, value)) = parse_option(line) else {
                comments.clear();
                continue;
            };
            match key.to_ascii_lowercase().as_str() {
                "host" => {
                    self.finish_block();
                    self.start_block(&value, &comments);
                }
                "match" => {
                    self.finish_block();
                    self.parsed.ignored.push(Ignored { pattern: line.to_string(), reason: "bloque Match" });
                }
                "include" => {
                    for pattern in value.split_whitespace() {
                        self.include(pattern, depth)?;
                    }
                }
                _ => {
                    if let Some(block) = &mut self.block {
                        apply_option(&mut block.data, key, value);
                    }
                }
            }
            comments.clear();
        }
        Ok(())
    }

    fn start_block(&mut self, patterns: &str, comments: &[&str]) {
        let mut aliases = Vec::new();
        for pattern in patterns.split_whitespace() {
            if pattern.contains(['*', '?', '!']) {
                self.parsed.ignored.push(Ignored { pattern: pattern.into(), reason: "patrón con comodines" });
            } else {
                aliases.push(pattern.to_string());
            }
        }
        if aliases.is_empty() {
            return;
        }
        let notes = comments.join("\n");
        let data = HostData {
            notes: (!notes.is_empty()).then_some(notes),
            ..Default::default()
        };
        self.block = Some(Block { aliases, data });
    }

    fn finish_block(&mut self) {
        let Some(block) = self.block.take() else { return };
        for alias in block.aliases {
            let mut data = HostData { alias: alias.clone(), ..block.data.clone() };
            if data.hostname.is_empty() {
                // Sin HostName, ssh conecta al propio nombre del Host.
                data.hostname = alias;
            }
            // Como ssh: si el alias aparece en varios bloques, gana el primer
            // valor de cada opción.
            match self.parsed.hosts.iter_mut().find(|h| h.alias == data.alias) {
                Some(existing) => merge(existing, data),
                None => self.parsed.hosts.push(data),
            }
        }
    }

    fn include(&mut self, pattern: &str, depth: usize) -> Result<()> {
        if depth >= MAX_INCLUDE_DEPTH {
            return Ok(());
        }
        let path = expand_home(pattern);
        let path = if path.is_absolute() { path } else { self.base.join(path) };
        for file in glob(&path) {
            if file.file_name().is_some_and(|n| n == GENERATED_FILE_NAME) {
                continue;
            }
            self.file(&file, depth + 1)?;
        }
        Ok(())
    }
}

fn apply_option(data: &mut HostData, key: String, value: String) {
    let first = |field: &mut Option<String>, value: String| {
        field.get_or_insert(value);
    };
    match key.to_ascii_lowercase().as_str() {
        "hostname" if data.hostname.is_empty() => data.hostname = value,
        "hostname" => {}
        "user" => first(&mut data.user, value),
        "proxyjump" => first(&mut data.proxy_jump, value),
        "port" => match value.parse() {
            Ok(port) => {
                data.port.get_or_insert(port);
            }
            Err(_) => data.extra_options.push(SshOption { key, value }),
        },
        "identityfile" if data.identity_file.is_none() => data.identity_file = Some(value),
        // Varias IdentityFile son válidas en ssh: las demás van como opciones.
        _ => data.extra_options.push(SshOption { key, value }),
    }
}

fn merge(into: &mut HostData, from: HostData) {
    into.user = into.user.take().or(from.user);
    into.port = into.port.or(from.port);
    into.identity_file = into.identity_file.take().or(from.identity_file);
    into.proxy_jump = into.proxy_jump.take().or(from.proxy_jump);
    into.notes = into.notes.take().or(from.notes);
    for opt in from.extra_options {
        if !into.extra_options.iter().any(|o| o.key.eq_ignore_ascii_case(&opt.key)) {
            into.extra_options.push(opt);
        }
    }
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new()
            .map(|d| d.home_dir().join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// Glob sencillo: admite `*` y `?` en el último componente de la ruta.
fn glob(path: &Path) -> Vec<PathBuf> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return vec![];
    };
    if !name.contains(['*', '?']) {
        return if path.is_file() { vec![path.to_path_buf()] } else { vec![] };
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let Ok(entries) = fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|n| wildcard_match(name, n)))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files
}

pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    // dp[j] = el prefijo del patrón procesado casa con t[..j]
    let mut dp = vec![false; t.len() + 1];
    dp[0] = true;
    for &pc in &p {
        let mut next = vec![false; t.len() + 1];
        next[0] = dp[0] && pc == '*';
        for j in 1..=t.len() {
            next[j] = match pc {
                '*' => next[j - 1] || dp[j],
                '?' => dp[j - 1],
                c => dp[j - 1] && t[j - 1] == c,
            };
        }
        dp = next;
    }
    dp[t.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "
# opciones globales: se ignoran
ServerAliveInterval 30

# Servidor de pruebas
# de OV
Host ovtest
  HostName 10.50.1.17
  User somadmin
  Port 2200
  IdentityFile ~/.ssh/a
  IdentityFile ~/.ssh/b
  LocalForward 8080 localhost:80

Host web1 web2
    HostName=web.example.com
    ProxyJump bastion

Host *.internal !foo
  User admin

Host bastion
  User=\"jump user\"

Match host x
  User nope

Host ovtest
  User otro
  ForwardAgent yes
";

    fn parsed() -> Parsed {
        parse_str(CONFIG, Path::new("/nonexistent"))
    }

    fn host<'a>(p: &'a Parsed, alias: &str) -> &'a HostData {
        p.hosts.iter().find(|h| h.alias == alias).unwrap()
    }

    #[test]
    fn concrete_hosts() {
        let p = parsed();
        let aliases: Vec<_> = p.hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["ovtest", "web1", "web2", "bastion"]);

        let ov = host(&p, "ovtest");
        assert_eq!(ov.hostname, "10.50.1.17");
        assert_eq!(ov.user.as_deref(), Some("somadmin"));
        assert_eq!(ov.port, Some(2200));
        assert_eq!(ov.identity_file.as_deref(), Some("~/.ssh/a"));
        assert_eq!(ov.notes.as_deref(), Some("Servidor de pruebas\nde OV"));
        let extra: Vec<_> = ov.extra_options.iter().map(|o| format!("{}={}", o.key, o.value)).collect();
        assert_eq!(extra, ["IdentityFile=~/.ssh/b", "LocalForward=8080 localhost:80", "ForwardAgent=yes"]);

        assert_eq!(host(&p, "web2").proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(host(&p, "web2").hostname, "web.example.com");
        // Sin HostName se usa el propio alias; las comillas se quitan.
        let bastion = host(&p, "bastion");
        assert_eq!((bastion.hostname.as_str(), bastion.user.as_deref()), ("bastion", Some("jump user")));
    }

    #[test]
    fn ignored_blocks() {
        let p = parsed();
        let ignored: Vec<_> = p.ignored.iter().map(|i| i.pattern.as_str()).collect();
        assert_eq!(ignored, ["*.internal", "!foo", "Match host x"]);
    }

    #[test]
    fn includes_with_glob() {
        let dir = std::env::temp_dir().join(format!("sshh-cfg-{}", std::process::id()));
        fs::create_dir_all(dir.join("config.d")).unwrap();
        fs::write(dir.join("config"), "Include config.d/*.conf\nHost main\n").unwrap();
        fs::write(dir.join("config.d/a.conf"), "Host a\n  User x\n").unwrap();
        fs::write(dir.join("config.d/b.txt"), "Host no\n").unwrap();
        fs::write(dir.join("config.d").join(GENERATED_FILE_NAME), "Host generated\n").unwrap();

        let p = parse_file(&dir.join("config")).unwrap();
        let aliases: Vec<_> = p.hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["a", "main"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*.conf", "a.conf"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("*.conf", "a.txt"));
        assert!(!wildcard_match("a?c", "ac"));
    }
}
