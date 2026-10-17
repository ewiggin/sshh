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
    /// What a drag does over a program that uses the mouse.
    #[serde(default)]
    pub mouse_drag: MouseDrag,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MouseDrag {
    /// A drag selects text with sshh; clicks and the wheel still go to the
    /// program, and Ctrl-drag sends the drag to it.
    #[default]
    Select,
    /// Everything goes to the program (Shift-drag: the terminal's selection).
    Program,
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
        assert_eq!(config, Config { theme: Some("monokai".into()), restore_panes: true, ..Config::default() });
        let config: Config = from_toml("mouse_drag = \"program\"\n").unwrap();
        assert_eq!(config.mouse_drag, MouseDrag::Program);
        assert!(from_toml::<Config>("mouse_drag = \"never\"\n").is_err());
        // Unknown keys are allowed (for settings of newer versions).
        assert!(from_toml::<Config>("future = 1\n").is_ok());
        let e = from_toml::<Config>("restore_panes = \"yes\"\n").unwrap_err();
        assert!(format!("{e:#}").starts_with("line 1:"), "{e:#}");
    }
}
