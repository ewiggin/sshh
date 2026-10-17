//! Color themes.
//!
//! The UI is drawn with the 16 ANSI colors (plus the default fg/bg), so with
//! the `terminal` theme it follows the terminal palette (Omarchy themes, …).
//! Any other theme is a palette applied after drawing (`Theme::apply`): the
//! default fg/bg and the ANSI colors become RGB, both in sshh's own UI (named
//! colors, `Color::Blue`…) and in the ssh panes (tui-term draws their colors
//! as `Color::Indexed`). 256-color and truecolor output of remote programs is
//! left as is.
//!
//! Themes are TOML files: built-in ones (`themes/` in the repository,
//! embedded) and the user's in `$XDG_CONFIG_HOME/sshh/themes/<name>.toml`,
//! which win over a built-in with the same name. The theme in use comes from
//! `$SSHH_THEME`, then `theme = "…"` in config.toml (`config`).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use serde::Deserialize;

use crate::config;

/// The theme that leaves the colors to the terminal.
pub const TERMINAL: &str = "terminal";

/// Built-in themes: (name, TOML source).
pub const BUILTIN: &[(&str, &str)] = &[
    ("everforest-dark", include_str!("../themes/everforest-dark.toml")),
    ("everforest-light", include_str!("../themes/everforest-light.toml")),
    ("gruvbox-dark", include_str!("../themes/gruvbox-dark.toml")),
    ("gruvbox-light", include_str!("../themes/gruvbox-light.toml")),
    ("kanagawa-dragon", include_str!("../themes/kanagawa-dragon.toml")),
    ("kanagawa-lotus", include_str!("../themes/kanagawa-lotus.toml")),
    ("kanagawa-wave", include_str!("../themes/kanagawa-wave.toml")),
    ("material", include_str!("../themes/material.toml")),
    ("material-palenight", include_str!("../themes/material-palenight.toml")),
    ("monokai", include_str!("../themes/monokai.toml")),
];

/// `terminal` has no source: it is the absence of a palette.
const TERMINAL_SOURCE: &str = "# The terminal's own colors (sshh's default): nothing to configure.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    fn parse(s: &str) -> Result<Self> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("'{s}' is not a color (#rrggbb)");
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
        Ok(Self(byte(0), byte(2), byte(4)))
    }
}

/// How the terminal shows RGB colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    /// Without truecolor the theme uses the nearest of the 256 colors.
    Ansi256,
}

impl Depth {
    /// From `$COLORTERM` (`truecolor` or `24bit`), as most terminals announce it.
    pub fn detect() -> Self {
        match std::env::var("COLORTERM").as_deref() {
            Ok("truecolor" | "24bit") => Self::TrueColor,
            _ => Self::Ansi256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Palette {
    background: Rgb,
    foreground: Rgb,
    ansi: [Rgb; 16],
    /// Overrides of sshh's own colors, by ANSI index (accent = 4…).
    ui: [Option<Rgb>; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    /// `None` for `terminal`.
    palette: Option<Palette>,
    depth: Depth,
}

/// The theme file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    background: String,
    foreground: String,
    palette: Vec<String>,
    #[serde(default)]
    ui: UiFile,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct UiFile {
    accent: Option<String>,
    #[serde(rename = "match")]
    match_: Option<String>,
    tag: Option<String>,
    connected: Option<String>,
    danger: Option<String>,
}

impl Theme {
    pub fn terminal() -> Self {
        Self { name: TERMINAL.into(), palette: None, depth: Depth::TrueColor }
    }

    pub fn parse(name: &str, source: &str, depth: Depth) -> Result<Self> {
        let file: ThemeFile = config::from_toml(source)?;
        let colors: Vec<Rgb> = file.palette.iter().map(|c| Rgb::parse(c)).collect::<Result<_>>()?;
        let ansi: [Rgb; 16] =
            colors.try_into().map_err(|c: Vec<Rgb>| anyhow!("palette has {} colors, it needs 16", c.len()))?;
        let mut ui = [None; 16];
        let UiFile { accent, match_, tag, connected, danger } = file.ui;
        for (index, color) in [(4, accent), (3, match_), (5, tag), (2, connected), (1, danger)] {
            if let Some(color) = color {
                ui[index] = Some(Rgb::parse(&color)?);
            }
        }
        let palette = Palette {
            background: Rgb::parse(&file.background)?,
            foreground: Rgb::parse(&file.foreground)?,
            ansi,
            ui,
        };
        Ok(Self { name: name.into(), palette: Some(palette), depth })
    }

    /// Recolors what was drawn in `buf` with the theme's palette.
    pub fn apply(&self, buf: &mut Buffer) {
        let Some(palette) = &self.palette else { return };
        for cell in &mut buf.content {
            cell.fg = self.map(palette, cell.fg, palette.foreground);
            cell.bg = self.map(palette, cell.bg, palette.background);
        }
    }

    fn map(&self, palette: &Palette, color: Color, default: Rgb) -> Color {
        let rgb = match color {
            Color::Reset => default,
            // Remote programs (through tui-term).
            Color::Indexed(i) if i < 16 => palette.ansi[usize::from(i)],
            Color::Indexed(_) => return color,
            Color::Rgb(r, g, b) => Rgb(r, g, b),
            // sshh's own UI.
            named => {
                let i = ansi_index(named);
                palette.ui[i].unwrap_or(palette.ansi[i])
            }
        };
        match self.depth {
            Depth::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
            Depth::Ansi256 => Color::Indexed(nearest_256(rgb)),
        }
    }
}

/// ANSI index of a named color.
fn ansi_index(color: Color) -> usize {
    match color {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        _ => 15,
    }
}

/// Nearest color of the xterm 256-color cube or gray ramp (16..=255; the
/// first 16 are the terminal palette, which is what the theme replaces).
fn nearest_256(Rgb(r, g, b): Rgb) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let dist = |(r2, g2, b2): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(r, r2) + d(g, g2) + d(b, b2)
    };
    let level = |v: u8| (0..6).min_by_key(|&i| (i32::from(LEVELS[i]) - i32::from(v)).abs()).unwrap_or(0);
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let cube_dist = dist((LEVELS[ri], LEVELS[gi], LEVELS[bi]));
    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gray = (average.saturating_sub(3) / 10).min(23) as usize;
    let value = (8 + 10 * gray) as u8;
    if dist((value, value, value)) < cube_dist { (232 + gray) as u8 } else { cube as u8 }
}

pub fn themes_dir() -> Result<PathBuf> {
    Ok(config::dir()?.join("themes"))
}

/// Name of the theme to use and where it comes from.
pub fn configured() -> Result<(String, &'static str)> {
    if let Some(name) = std::env::var("SSHH_THEME").ok().filter(|n| !n.is_empty()) {
        return Ok((name, "$SSHH_THEME"));
    }
    Ok(match config::load()?.theme {
        Some(name) => (name, "config.toml"),
        None => (TERMINAL.into(), "the default"),
    })
}

/// Where a theme comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Terminal,
    Builtin(&'static str),
    /// A file in the themes folder (`overrides`: it has a built-in's name).
    User { path: PathBuf, overrides: bool },
}

impl Source {
    pub fn read(&self) -> Result<String> {
        match self {
            Self::Terminal => Ok(TERMINAL_SOURCE.into()),
            Self::Builtin(source) => Ok((*source).into()),
            Self::User { path, .. } => {
                fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
            }
        }
    }
}

/// Every theme by name: `terminal`, the built-in ones and the user's (which
/// replace a built-in with the same name).
pub fn list() -> Result<Vec<(String, Source)>> {
    list_in(&themes_dir()?)
}

fn list_in(dir: &Path) -> Result<Vec<(String, Source)>> {
    let mut themes: Vec<(String, Source)> =
        BUILTIN.iter().map(|(name, source)| (name.to_string(), Source::Builtin(source))).collect();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else { continue };
        if name == TERMINAL {
            continue;
        }
        match themes.iter_mut().find(|(n, _)| *n == name) {
            Some((_, source)) => *source = Source::User { path, overrides: true },
            None => themes.push((name, Source::User { path, overrides: false })),
        }
    }
    themes.sort_by(|a, b| a.0.cmp(&b.0));
    themes.insert(0, (TERMINAL.into(), Source::Terminal));
    Ok(themes)
}

/// Loads a theme by name.
pub fn load(name: &str, depth: Depth) -> Result<Theme> {
    let themes = list()?;
    let Some((_, source)) = themes.iter().find(|(n, _)| n == name) else {
        bail!("no theme named '{name}' (see `sshh themes`)");
    };
    from_source(name, source, depth)
}

pub fn from_source(name: &str, source: &Source, depth: Depth) -> Result<Theme> {
    match source {
        Source::Terminal => Ok(Theme::terminal()),
        Source::Builtin(text) => Theme::parse(name, text, depth),
        Source::User { path, .. } => {
            Theme::parse(name, &source.read()?, depth).with_context(|| format!("theme {}", path.display()))
        }
    }
}

/// The configured theme. A theme that can't be loaded never stops sshh: it
/// falls back to `terminal` and the error is returned to be shown.
pub fn current() -> (Theme, Option<String>) {
    match configured().and_then(|(name, _)| load(&name, Depth::detect())) {
        Ok(theme) => (theme, None),
        Err(e) => (Theme::terminal(), Some(format!("{e:#}; using the terminal colors"))),
    }
}

/// Sets `theme = "<name>"` in config.toml, keeping the rest of the file.
pub fn save(name: &str) -> Result<PathBuf> {
    let path = config::file()?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    fs::write(&path, set_theme(&text, name)).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// `text` with its top-level `theme = …` line set to `name` (added at the top
/// if there is none).
fn set_theme(text: &str, name: &str) -> String {
    let line = format!("theme = {}", toml::Value::String(name.into()));
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    // Top-level keys go before the first table.
    let top = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let existing = lines[..top].iter().position(|l| {
        l.trim_start().strip_prefix("theme").is_some_and(|rest| rest.trim_start().starts_with('='))
    });
    match existing {
        Some(i) => lines[i] = line,
        None => lines.insert(0, line),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn builtin_themes_parse() {
        for (name, source) in BUILTIN {
            let theme = Theme::parse(name, source, Depth::TrueColor).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert!(theme.palette.is_some());
        }
    }

    #[test]
    fn invalid_themes_are_rejected() {
        let ok = "background = \"#000000\"\nforeground = \"#ffffff\"\n";
        let sixteen = format!("palette = [{}]\n", vec!["\"#101010\""; 16].join(", "));
        assert!(Theme::parse("x", &format!("{ok}{sixteen}"), Depth::TrueColor).is_ok());
        let short = format!("{ok}palette = [\"#101010\"]\n");
        let e = Theme::parse("x", &short, Depth::TrueColor).unwrap_err();
        assert!(format!("{e:#}").contains("needs 16"), "{e:#}");
        let bad = format!("{ok}{sixteen}[ui]\naccent = \"blue\"\n");
        assert!(Theme::parse("x", &bad, Depth::TrueColor).is_err());
        let unknown = format!("{ok}{sixteen}colour = \"#000000\"\n");
        let e = Theme::parse("x", &unknown, Depth::TrueColor).unwrap_err();
        // One line, so it fits in the TUI footer.
        assert!(format!("{e:#}").starts_with("line 4: unknown field `colour`"), "{e:#}");
    }

    fn sample(depth: Depth) -> Theme {
        let palette: Vec<String> = (0..16).map(|i| format!("\"#0000{i:02x}\"")).collect();
        let source = format!(
            "background = \"#101010\"\nforeground = \"#f0f0f0\"\npalette = [{}]\n[ui]\naccent = \"#abcdef\"\n",
            palette.join(", ")
        );
        Theme::parse("sample", &source, depth).unwrap()
    }

    #[test]
    fn apply_maps_defaults_ansi_and_ui_colors() {
        let theme = sample(Depth::TrueColor);
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 1));
        let colors = [
            (Color::Reset, Color::Reset),
            (Color::Indexed(4), Color::Indexed(1)),
            (Color::Blue, Color::Red),
            (Color::Indexed(100), Color::Rgb(1, 2, 3)),
        ];
        for (x, (fg, bg)) in colors.iter().enumerate() {
            let cell = &mut buf[(x as u16, 0)];
            cell.fg = *fg;
            cell.bg = *bg;
        }
        theme.apply(&mut buf);
        let cell = |x: u16| (buf[(x, 0)].fg, buf[(x, 0)].bg);
        assert_eq!(cell(0), (Color::Rgb(0xf0, 0xf0, 0xf0), Color::Rgb(0x10, 0x10, 0x10)));
        // A remote program's blue is the palette's; sshh's blue (accent) the override.
        assert_eq!(cell(1), (Color::Rgb(0, 0, 4), Color::Rgb(0, 0, 1)));
        assert_eq!(cell(2), (Color::Rgb(0xab, 0xcd, 0xef), Color::Rgb(0, 0, 1)));
        // 256-color and truecolor output is untouched.
        assert_eq!(cell(3), (Color::Indexed(100), Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn terminal_theme_changes_nothing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf[(1, 0)].fg = Color::Blue;
        let before = buf.clone();
        Theme::terminal().apply(&mut buf);
        assert_eq!(buf, before);
    }

    #[test]
    fn without_truecolor_uses_the_256_colors() {
        let theme = sample(Depth::Ansi256);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        theme.apply(&mut buf);
        assert!(matches!(buf[(0, 0)].fg, Color::Indexed(i) if i >= 16));
        assert_eq!(nearest_256(Rgb(255, 0, 0)), 196);
        assert_eq!(nearest_256(Rgb(0, 0, 0)), 16);
        assert_eq!(nearest_256(Rgb(128, 128, 128)), 244);
        assert_eq!(nearest_256(Rgb(0x28, 0x28, 0x28)), 235);
    }

    #[test]
    fn user_themes_override_builtins() {
        let dir = std::env::temp_dir().join(format!("sshh-themes-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("monokai.toml"), "x").unwrap();
        fs::write(dir.join("mine.toml"), "x").unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        let themes = list_in(&dir).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(themes[0], (TERMINAL.to_string(), Source::Terminal));
        let find = |name: &str| themes.iter().find(|(n, _)| n == name).map(|(_, s)| s.clone());
        assert!(matches!(find("monokai"), Some(Source::User { overrides: true, .. })));
        assert!(matches!(find("mine"), Some(Source::User { overrides: false, .. })));
        assert!(matches!(find("gruvbox-dark"), Some(Source::Builtin(_))));
        assert_eq!(find("notes"), None);
        assert_eq!(themes.len(), BUILTIN.len() + 2);
    }

    #[test]
    fn set_theme_keeps_the_rest_of_the_file() {
        assert_eq!(set_theme("", "monokai"), "theme = \"monokai\"\n");
        assert_eq!(
            set_theme("# mine\ntheme = \"old\"\nother = 1\n", "gruvbox-dark"),
            "# mine\ntheme = \"gruvbox-dark\"\nother = 1\n"
        );
        // A `theme` key inside a table is not the top-level one.
        assert_eq!(set_theme("[x]\ntheme = 1\n", "a"), "theme = \"a\"\n[x]\ntheme = 1\n");
        let config: crate::config::Config = toml::from_str(&set_theme("# c\n", "m")).unwrap();
        assert_eq!(config.theme.as_deref(), Some("m"));
    }
}
