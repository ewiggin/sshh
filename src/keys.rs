//! Pane shortcuts: the actions, their default keys and the user's remapping
//! (`[keys]` in config.toml: `action = "alt-x"` or a list of keys).

use std::collections::BTreeMap;
use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    FocusLeft,
    FocusDown,
    FocusUp,
    FocusRight,
    /// Jump to column N (1..=9).
    Column(u8),
    NextSession,
    PrevSession,
    NewColumn,
    NewPaneBelow,
    History,
    ClosePane,
    Zoom,
    Broadcast,
    MoveColumnLeft,
    MoveColumnRight,
    Stack,
    Unstack,
}

impl Action {
    pub fn all() -> Vec<Self> {
        let mut all = vec![Self::FocusLeft, Self::FocusDown, Self::FocusUp, Self::FocusRight];
        all.extend((1..=9).map(Self::Column));
        all.extend([
            Self::NextSession,
            Self::PrevSession,
            Self::NewColumn,
            Self::NewPaneBelow,
            Self::History,
            Self::ClosePane,
            Self::Zoom,
            Self::Broadcast,
            Self::MoveColumnLeft,
            Self::MoveColumnRight,
            Self::Stack,
            Self::Unstack,
        ]);
        all
    }

    pub fn name(self) -> String {
        match self {
            Self::FocusLeft => "focus-left".into(),
            Self::FocusDown => "focus-down".into(),
            Self::FocusUp => "focus-up".into(),
            Self::FocusRight => "focus-right".into(),
            Self::Column(n) => format!("column-{n}"),
            Self::NextSession => "next-session".into(),
            Self::PrevSession => "prev-session".into(),
            Self::NewColumn => "new-column".into(),
            Self::NewPaneBelow => "new-pane-below".into(),
            Self::History => "history".into(),
            Self::ClosePane => "close-pane".into(),
            Self::Zoom => "zoom".into(),
            Self::Broadcast => "broadcast".into(),
            Self::MoveColumnLeft => "move-column-left".into(),
            Self::MoveColumnRight => "move-column-right".into(),
            Self::Stack => "stack".into(),
            Self::Unstack => "unstack".into(),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|a| a.name() == name)
    }

    pub fn description(self) -> String {
        match self {
            Self::FocusLeft => "focus the pane on the left (or the list)".into(),
            Self::FocusDown => "focus the pane below (in the list: next connection with a session)".into(),
            Self::FocusUp => "focus the pane above (in the list: previous connection with a session)".into(),
            Self::FocusRight => "focus the pane on the right".into(),
            Self::Column(n) => format!("jump to column {n}"),
            Self::NextSession => "next session in the pane".into(),
            Self::PrevSession => "previous session in the pane".into(),
            Self::NewColumn => "show the selection in a new column".into(),
            Self::NewPaneBelow => "show the selection in a new pane below the last column".into(),
            Self::History => "history mode".into(),
            Self::ClosePane => "close the pane".into(),
            Self::Zoom => "full screen for the active pane".into(),
            Self::Broadcast => "broadcast: type in every visible pane".into(),
            Self::MoveColumnLeft => "move the column left".into(),
            Self::MoveColumnRight => "move the column right".into(),
            Self::Stack => "stack the pane below the next column".into(),
            Self::Unstack => "take the pane out into its own column".into(),
        }
    }

    fn defaults(self) -> Vec<Key> {
        let alt = |code| Key { code, modifiers: KeyModifiers::ALT };
        let ch = |c| alt(KeyCode::Char(c));
        match self {
            Self::FocusLeft => vec![ch('h'), alt(KeyCode::Left)],
            Self::FocusDown => vec![ch('j'), alt(KeyCode::Down)],
            Self::FocusUp => vec![ch('k'), alt(KeyCode::Up)],
            Self::FocusRight => vec![ch('l'), alt(KeyCode::Right)],
            Self::Column(n) => vec![ch(char::from(b'0' + n))],
            Self::NextSession => vec![ch('n')],
            Self::PrevSession => vec![ch('p')],
            Self::NewColumn => vec![ch('v')],
            Self::NewPaneBelow => vec![ch('-')],
            Self::History => vec![ch('s')],
            Self::ClosePane => vec![ch('w')],
            Self::Zoom => vec![ch('f'), ch('z')],
            Self::Broadcast => vec![ch('a')],
            Self::MoveColumnLeft => vec![ch('H')],
            Self::MoveColumnRight => vec![ch('L')],
            Self::Stack => vec![ch('J')],
            Self::Unstack => vec![ch('K')],
        }
    }
}

/// A key with Ctrl/Alt (Shift is folded into uppercase letters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl Key {
    /// Parses `alt-h`, `ctrl-shift-left`, `alt-H` (= `alt-shift-h`), `alt--`,
    /// `f5`… Case-insensitive except for a single letter, where uppercase
    /// means Shift. It needs Ctrl or Alt (or to be F1–F12), so that it never
    /// takes a key that is typed.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut rest = spec.trim();
        let mut modifiers = KeyModifiers::NONE;
        loop {
            let lower = rest.to_ascii_lowercase();
            let (modifier, len) = if lower.starts_with("ctrl-") {
                (KeyModifiers::CONTROL, 5)
            } else if lower.starts_with("alt-") {
                (KeyModifiers::ALT, 4)
            } else if lower.starts_with("meta-") {
                (KeyModifiers::ALT, 5)
            } else if lower.starts_with("shift-") {
                (KeyModifiers::SHIFT, 6)
            } else {
                break;
            };
            // `alt--` is Alt and the `-` key.
            if rest.len() == len {
                break;
            }
            modifiers |= modifier;
            rest = &rest[len..];
        }
        let lower = rest.to_ascii_lowercase();
        let code = match lower.as_str() {
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "tab" => KeyCode::Tab,
            "enter" => KeyCode::Enter,
            "space" => KeyCode::Char(' '),
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => {
                KeyCode::F(f[1..].parse().unwrap_or(1))
            }
            _ => {
                let mut chars = rest.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if !c.is_whitespace() => KeyCode::Char(c),
                    _ => return Err(format!("'{spec}' is not a key")),
                }
            }
        };
        let key = Self::new(code, modifiers);
        let function = matches!(key.code, KeyCode::F(_));
        if !function && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return Err(format!("'{spec}' needs ctrl- or alt- (it would take a key you type)"));
        }
        Ok(key)
    }

    /// Normalized: a letter with Shift is the uppercase letter without it.
    fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        match code {
            KeyCode::Char(c) if modifiers.contains(KeyModifiers::SHIFT) || c.is_uppercase() => {
                Self { code: KeyCode::Char(c.to_ascii_uppercase()), modifiers: modifiers - KeyModifiers::SHIFT }
            }
            _ => Self { code, modifiers },
        }
    }

    fn of(event: KeyEvent) -> Self {
        let modifiers = event.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        Self::new(event.code, modifiers)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            f.write_str("Ctrl-")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            f.write_str("Alt-")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            f.write_str("Shift-")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Left => f.write_str("←"),
            KeyCode::Right => f.write_str("→"),
            KeyCode::Up => f.write_str("↑"),
            KeyCode::Down => f.write_str("↓"),
            KeyCode::F(n) => write!(f, "F{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// `[keys]` values: one key or several.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum KeyList {
    One(String),
    Many(Vec<String>),
}

impl KeyList {
    fn specs(&self) -> Vec<&str> {
        match self {
            Self::One(key) => vec![key.as_str()],
            Self::Many(keys) => keys.iter().map(String::as_str).collect(),
        }
    }
}

/// The effective keys of every action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: BTreeMap<Action, Vec<Key>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self { bindings: Action::all().into_iter().map(|a| (a, a.defaults())).collect() }
    }
}

impl Keymap {
    /// The defaults with the user's `[keys]` applied, and what was wrong with
    /// them (it never fails: a bad entry keeps that action's default keys).
    /// A key the user gives an action is taken away from the action that had
    /// it by default; two of the user's actions with the same key keep their
    /// defaults.
    pub fn from_config(keys: &BTreeMap<String, KeyList>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut custom: BTreeMap<Action, Vec<Key>> = BTreeMap::new();
        for (name, list) in keys {
            let Some(action) = Action::from_name(name) else {
                warnings.push(format!("[keys]: unknown action '{name}'"));
                continue;
            };
            match list.specs().into_iter().map(Key::parse).collect::<Result<Vec<_>, _>>() {
                Ok(parsed) => {
                    custom.insert(action, parsed);
                }
                Err(e) => warnings.push(format!("[keys] {name}: {e}; using the default")),
            }
        }
        // Two of the user's actions with the same key: both keep their defaults.
        let clashes: Vec<(Action, Action, Key)> = custom
            .iter()
            .flat_map(|(a, keys)| keys.iter().map(move |k| (*a, *k)))
            .flat_map(|(a, k)| {
                custom.iter().filter(move |(b, keys)| a < **b && keys.contains(&k)).map(move |(b, _)| (a, *b, k))
            })
            .collect();
        for (a, b, key) in clashes {
            warnings.push(format!("[keys]: '{key}' is set for '{}' and '{}'; both keep their defaults", a.name(), b.name()));
            custom.remove(&a);
            custom.remove(&b);
        }
        let mut map = Self::default();
        for (action, keys) in &custom {
            for other in map.bindings.values_mut() {
                other.retain(|k| !keys.contains(k));
            }
            map.bindings.insert(*action, keys.clone());
        }
        (map, warnings)
    }

    pub fn action(&self, event: KeyEvent) -> Option<Action> {
        let key = Key::of(event);
        self.bindings.iter().find(|(_, keys)| keys.contains(&key)).map(|(a, _)| *a)
    }

    pub fn keys(&self, action: Action) -> &[Key] {
        self.bindings.get(&action).map_or(&[], Vec::as_slice)
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.keys(action) == action.defaults().as_slice()
    }

    /// The label for some actions in the help or the footer: `default` while
    /// they keep their default keys, otherwise their first keys joined by
    /// `/` (`-` if an action has none).
    pub fn label(&self, actions: &[Action], default: &str) -> String {
        if actions.iter().all(|a| self.is_default(*a)) {
            return default.to_string();
        }
        actions
            .iter()
            .map(|a| self.keys(*a).first().map_or_else(|| "-".to_string(), Key::to_string))
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Like `label`, for messages about one action ("Alt-a stops it").
    pub fn first(&self, action: Action) -> String {
        self.keys(action).first().map_or_else(|| action.name(), Key::to_string)
    }
}

/// `config.toml` as `sshh config init` writes it: every setting commented out,
/// with its default.
pub fn default_config_file() -> String {
    let mut out = String::from(
        "# sshh settings. Everything here is optional; these are the defaults.\n\
         \n\
         # Theme: `sshh themes` lists them, `T` in the TUI picks one ($SSHH_THEME wins).\n\
         # theme = \"terminal\"\n\
         \n\
         # Reopen the panes of the last session when sshh starts, without connecting\n\
         # (a default layout, `*` in the Layouts tab, wins over this).\n\
         # restore_panes = false\n\
         \n\
         # Pane shortcuts: each action takes a key or a list of keys. Keys need ctrl- or alt-\n\
         # (or are f1…f12): alt-h, ctrl-alt-left, alt-H (= alt-shift-h), alt--.\n\
         # Setting an action replaces its default keys.\n\
         [keys]\n",
    );
    for action in Action::all() {
        let keys: Vec<String> = action.defaults().iter().map(spec).collect();
        let value = match keys.as_slice() {
            [one] => format!("\"{one}\""),
            many => format!("[{}]", many.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ")),
        };
        out.push_str(&format!("# {} = {value}\n", action.name()));
    }
    out
}

/// A key as written in config.toml (the inverse of `Key::parse`).
pub fn spec(key: &Key) -> String {
    let mut out = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        out.push_str("ctrl-");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        out.push_str("alt-");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        out.push_str("shift-");
    }
    match key.code {
        KeyCode::Char(' ') => out.push_str("space"),
        KeyCode::Char(c) => out.push(c),
        KeyCode::F(n) => out.push_str(&format!("f{n}")),
        other => out.push_str(&format!("{other:?}").to_lowercase()),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn keys(entries: &[(&str, KeyList)]) -> BTreeMap<String, KeyList> {
        entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn one(key: &str) -> KeyList {
        KeyList::One(key.into())
    }

    #[test]
    fn parse_keys() {
        let alt = KeyModifiers::ALT;
        assert_eq!(Key::parse("alt-h"), Ok(Key { code: KeyCode::Char('h'), modifiers: alt }));
        assert_eq!(Key::parse("ALT-Left"), Ok(Key { code: KeyCode::Left, modifiers: alt }));
        // Shift with a letter is the uppercase letter.
        assert_eq!(Key::parse("alt-shift-h"), Key::parse("alt-H"));
        assert_eq!(Key::parse("alt--"), Ok(Key { code: KeyCode::Char('-'), modifiers: alt }));
        assert_eq!(
            Key::parse("ctrl-shift-left"),
            Ok(Key { code: KeyCode::Left, modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT })
        );
        assert_eq!(Key::parse("f5"), Ok(Key { code: KeyCode::F(5), modifiers: KeyModifiers::NONE }));
        for bad in ["h", "shift-h", "alt-", "alt-hello", "f13", ""] {
            assert!(Key::parse(bad).is_err(), "{bad}");
        }
        // `spec` writes what `parse` reads.
        for action in Action::all() {
            for key in action.defaults() {
                assert_eq!(Key::parse(&spec(&key)), Ok(key), "{}", spec(&key));
            }
        }
    }

    #[test]
    fn default_keymap() {
        let map = Keymap::default();
        assert_eq!(map.action(event(KeyCode::Char('h'), KeyModifiers::ALT)), Some(Action::FocusLeft));
        assert_eq!(map.action(event(KeyCode::Left, KeyModifiers::ALT)), Some(Action::FocusLeft));
        assert_eq!(map.action(event(KeyCode::Char('3'), KeyModifiers::ALT)), Some(Action::Column(3)));
        // Terminals send Alt-Shift-h as 'H', with or without SHIFT.
        assert_eq!(map.action(event(KeyCode::Char('H'), KeyModifiers::ALT)), Some(Action::MoveColumnLeft));
        assert_eq!(
            map.action(event(KeyCode::Char('H'), KeyModifiers::ALT | KeyModifiers::SHIFT)),
            Some(Action::MoveColumnLeft)
        );
        assert_eq!(map.action(event(KeyCode::Char('h'), KeyModifiers::NONE)), None);
        assert_eq!(map.action(event(KeyCode::Char('b'), KeyModifiers::ALT)), None);
        assert_eq!(map.label(&[Action::Zoom], "Alt-f / Alt-z"), "Alt-f / Alt-z");
        // Every action has a distinct name and its own keys.
        let names: std::collections::HashSet<String> = Action::all().iter().map(|a| a.name()).collect();
        assert_eq!(names.len(), Action::all().len());
    }

    #[test]
    fn remapping() {
        let (map, warnings) = Keymap::from_config(&keys(&[
            ("zoom", one("ctrl-alt-z")),
            ("focus-left", KeyList::Many(vec!["alt-y".into(), "ctrl-left".into()])),
            ("broadcast", one("alt-h")),
        ]));
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(map.action(event(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::ALT)), Some(Action::Zoom));
        // The old keys of a remapped action are free (they go to ssh).
        assert_eq!(map.action(event(KeyCode::Char('f'), KeyModifiers::ALT)), None);
        assert_eq!(map.action(event(KeyCode::Char('y'), KeyModifiers::ALT)), Some(Action::FocusLeft));
        // alt-h was focus-left's default; now it's broadcast, and alt-a is free.
        assert_eq!(map.action(event(KeyCode::Char('h'), KeyModifiers::ALT)), Some(Action::Broadcast));
        assert_eq!(map.action(event(KeyCode::Char('a'), KeyModifiers::ALT)), None);
        assert_eq!(map.label(&[Action::Zoom], "Alt-f / Alt-z"), "Ctrl-Alt-z");
        assert_eq!(map.first(Action::Broadcast), "Alt-h");
    }

    #[test]
    fn bad_entries_keep_the_defaults() {
        let (map, warnings) = Keymap::from_config(&keys(&[
            ("zoom", one("z")),
            ("teleport", one("alt-t")),
            ("history", one("alt-x")),
            ("close-pane", one("alt-x")),
        ]));
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[0].contains("unknown action 'teleport'"), "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("zoom") && w.contains("needs ctrl- or alt-")));
        assert!(warnings.iter().any(|w| w.contains("'Alt-x'") && w.contains("both keep their defaults")));
        assert_eq!(map, Keymap::default());
    }

    #[test]
    fn default_config_file_parses() {
        let text = default_config_file();
        let config: crate::config::Config = crate::config::from_toml(&text).unwrap();
        assert_eq!(config, crate::config::Config::default());
        // Uncommenting every key gives the defaults.
        let uncommented: String =
            text.lines().map(|l| l.strip_prefix("# ").filter(|l| l.contains(" = ")).unwrap_or(l)).collect::<Vec<_>>().join("\n");
        let config: crate::config::Config = crate::config::from_toml(&uncommented).unwrap();
        let (map, warnings) = Keymap::from_config(&config.keys);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map, Keymap::default());
    }
}
