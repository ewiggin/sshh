//! sshh's settings: `$XDG_CONFIG_HOME/sshh/config.toml` (default
//! `~/.config/sshh/config.toml`, also on macOS). A missing file means the
//! defaults; a broken one is reported and never stops sshh.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// Theme name (see `theme`); `$SSHH_THEME` wins over it.
    pub theme: Option<String>,
    /// Reopen the panes of the last session when the TUI starts (without
    /// connecting), unless there is a default layout.
    #[serde(default)]
    pub restore_panes: bool,
    /// Order of the list: `frecency`, `recent`, `most-used` or `alphabetical`.
    pub sort: Option<String>,
}

/// `$XDG_CONFIG_HOME/sshh`.
pub fn dir() -> Result<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join(".config")))
        .ok_or_else(|| anyhow!("could not determine the config directory"))?;
    Ok(config.join("sshh"))
}

pub fn file() -> Result<PathBuf> {
    Ok(dir()?.join("config.toml"))
}

pub fn load() -> Result<Config> {
    let path = file()?;
    match fs::read_to_string(&path) {
        Ok(text) => from_toml(&text).with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Sets the top-level `key = "<value>"` in config.toml, keeping the rest of
/// the file.
pub fn save_key(key: &str, value: &str) -> Result<PathBuf> {
    let path = file()?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    fs::write(&path, set_key(&text, key, value)).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// `text` with its top-level `key = …` line set to `value` (added at the top
/// if there is none).
fn set_key(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {}", toml::Value::String(value.into()));
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    // Top-level keys go before the first table.
    let top = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let existing = lines[..top].iter().position(|l| {
        l.trim_start().strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))
    });
    match existing {
        Some(i) => lines[i] = line,
        None => lines.insert(0, line),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Parses TOML with a one-line error (the TUI shows it in the footer).
pub fn from_toml<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    toml::from_str(text).map_err(|e| {
        let message = e.message().trim().replace('\n', " ");
        match e.span() {
            Some(span) => anyhow!("line {}: {message}", text[..span.start].matches('\n').count() + 1),
            None => anyhow!("{message}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_settings() {
        assert_eq!(from_toml::<Config>("").unwrap(), Config::default());
        let config: Config = from_toml("theme = \"monokai\"\nrestore_panes = true\n").unwrap();
        assert_eq!(config, Config { theme: Some("monokai".into()), restore_panes: true, sort: None });
        // Unknown keys are allowed (for settings of newer versions).
        assert!(from_toml::<Config>("future = 1\n").is_ok());
        let e = from_toml::<Config>("restore_panes = \"yes\"\n").unwrap_err();
        assert!(format!("{e:#}").starts_with("line 1:"), "{e:#}");
    }

    #[test]
    fn set_key_keeps_the_rest_of_the_file() {
        assert_eq!(set_key("", "theme", "monokai"), "theme = \"monokai\"\n");
        assert_eq!(
            set_key("# mine\ntheme = \"old\"\nother = 1\n", "theme", "gruvbox-dark"),
            "# mine\ntheme = \"gruvbox-dark\"\nother = 1\n"
        );
        // A key inside a table is not the top-level one.
        assert_eq!(set_key("[x]\ntheme = 1\n", "theme", "a"), "theme = \"a\"\n[x]\ntheme = 1\n");
        // `sort` is not `sorted`.
        assert_eq!(set_key("sorted = 1\n", "sort", "recent"), "sort = \"recent\"\nsorted = 1\n");
        let text = set_key(&set_key("# c\n", "theme", "m"), "sort", "frecency");
        let config: Config = from_toml(&text).unwrap();
        assert_eq!(config.theme.as_deref(), Some("m"));
        assert_eq!(config.sort.as_deref(), Some("frecency"));
    }
}
