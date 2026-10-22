use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Extra ssh_config option (`Key Value`) attached to a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshOption {
    pub key: String,
    pub value: String,
}

/// Editable data of a connection. Also the import/export format.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostData {
    pub alias: String,
    pub hostname: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<String>,
    /// Remote tmux session to attach to (or create) in interactive sessions,
    /// so that it survives sshh and the network.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_options: Vec<SshOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Shown at the top of the list.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

impl HostData {
    pub fn validate(&self) -> Result<()> {
        validate_alias(&self.alias)?;
        if self.hostname.is_empty() || self.hostname.chars().any(char::is_whitespace) {
            bail!("invalid hostname '{}'", self.hostname);
        }
        if self.port == Some(0) {
            bail!("port cannot be 0");
        }
        if let Some(name) = &self.tmux_session {
            validate_tmux_session(name)?;
        }
        Ok(())
    }

    /// Human readable target: `user@hostname:port`.
    pub fn target(&self) -> String {
        let mut s = String::new();
        if let Some(user) = &self.user {
            s.push_str(user);
            s.push('@');
        }
        s.push_str(&self.hostname);
        if let Some(port) = self.port {
            s.push_str(&format!(":{port}"));
        }
        s
    }

    /// ssh_config options describing the connection, in order.
    pub fn ssh_options(&self) -> Vec<(String, String)> {
        let mut opts = vec![("HostName".to_string(), self.hostname.clone())];
        let fields = [
            ("User", self.user.clone()),
            ("Port", self.port.map(|p| p.to_string())),
            ("IdentityFile", self.identity_file.clone()),
            ("ProxyJump", self.proxy_jump.clone()),
        ];
        opts.extend(
            fields
                .into_iter()
                .filter_map(|(k, v)| v.map(|v| (k.to_string(), v))),
        );
        for opt in &self.extra_options {
            if !opts.iter().any(|(k, _)| k.eq_ignore_ascii_case(&opt.key)) {
                opts.push((opt.key.clone(), opt.value.clone()));
            }
        }
        opts
    }
}

/// ssh_config options whose value is a single argument (paths, names).
/// Others may take several (`LocalForward 8080 localhost:80`) and must not
/// be quoted.
const SINGLE_VALUE_OPTIONS: &[&str] = &[
    "hostname",
    "user",
    "identityfile",
    "certificatefile",
    "identityagent",
    "controlpath",
    "securitykeyprovider",
    "pkcs11provider",
    "xauthlocation",
];

/// Option value as it must be written in ssh_config or in `-o`.
pub fn config_value(key: &str, value: &str) -> String {
    let single = SINGLE_VALUE_OPTIONS.contains(&key.to_ascii_lowercase().as_str());
    if single && value.chars().any(char::is_whitespace) && !value.starts_with('"') {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

/// Connection stored in the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: i64,
    pub data: HostData,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used: Option<i64>,
    pub use_count: i64,
}

/// sshh's own subcommands; they cannot be used as aliases.
pub const RESERVED_ALIASES: &[&str] =
    &["ls", "add", "rm", "help", "history", "export", "import", "import-ssh-config", "ssh-config", "themes", "ignore"];

/// tmux session names: letters, digits, `-` and `_`, so they need no quoting
/// in the remote command (tmux itself rejects `:` and `.`).
pub fn validate_tmux_session(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("tmux session cannot be empty");
    }
    if let Some(c) = name.chars().find(|c| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_')) {
        bail!("tmux session cannot contain '{c}' (use letters, digits, '-' and '_')");
    }
    Ok(())
}

/// Remote command that attaches to the tmux session `name`, creating it if
/// needed. Without tmux on the server it falls back to a login shell.
/// The user ssh uses when none is given.
pub fn local_user() -> String {
    std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default()
}

/// Where ssh connects: `user@hostname:port`, always explicit (the form the
/// ignored destinations are stored and shown in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub user: String,
    pub hostname: String,
    pub port: u16,
}

impl Endpoint {
    /// Parses exactly `user@hostname:port` (as `sshh ignore` prints it).
    pub fn parse(s: &str) -> Option<Self> {
        let (user, rest) = s.split_once('@')?;
        let (hostname, port) = rest.rsplit_once(':')?;
        let port = port.parse().ok()?;
        let valid = |part: &str| !part.is_empty() && !part.contains(['@', ':', '/']);
        (valid(user) && valid(hostname))
            .then(|| Self { user: user.into(), hostname: hostname.into(), port })
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}:{}", self.user, self.hostname, self.port)
    }
}

pub fn tmux_command(name: &str) -> String {
    format!("tmux new-session -A -s {name} || exec $SHELL -l")
}

pub fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty() {
        bail!("alias cannot be empty");
    }
    if alias.starts_with('-') {
        bail!("alias cannot start with '-'");
    }
    if let Some(c) = alias
        .chars()
        .find(|c| c.is_whitespace() || matches!(c, '*' | '?' | '!' | ',' | '@' | '"' | '#'))
    {
        bail!("alias cannot contain '{c}'");
    }
    if RESERVED_ALIASES.contains(&alias) {
        bail!("'{alias}' is an sshh subcommand");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_validation() {
        assert!(validate_alias("web-1").is_ok());
        for bad in ["", "-x", "a b", "a*", "ls", "u@h"] {
            assert!(validate_alias(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn config_values() {
        assert_eq!(config_value("IdentityFile", "~/my key"), "\"~/my key\"");
        assert_eq!(config_value("LocalForward", "8080 localhost:80"), "8080 localhost:80");
        assert_eq!(config_value("User", "root"), "root");
    }

    #[test]
    fn ssh_options_skip_duplicates() {
        let data = HostData {
            alias: "a".into(),
            hostname: "h".into(),
            port: Some(22),
            extra_options: vec![
                SshOption { key: "port".into(), value: "1".into() },
                SshOption { key: "ForwardAgent".into(), value: "yes".into() },
            ],
            ..Default::default()
        };
        let keys: Vec<_> = data.ssh_options().into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(keys, ["HostName=h", "Port=22", "ForwardAgent=yes"]);
    }

    #[test]
    fn endpoint_parse() {
        let e = Endpoint::parse("deploy@10.0.0.5:2222").unwrap();
        assert_eq!((e.user.as_str(), e.hostname.as_str(), e.port), ("deploy", "10.0.0.5", 2222));
        assert_eq!(e.to_string(), "deploy@10.0.0.5:2222");
        for s in ["10.0.0.5:22", "deploy@10.0.0.5", "@h:22", "u@:22", "u@h:x", "u@h:99999", "ssh://u@h:22", "u@[::1]:22"] {
            assert_eq!(Endpoint::parse(s), None, "{s}");
        }
    }

    #[test]
    fn pinned_is_omitted_from_json_when_false() {
        let data = HostData { alias: "a".into(), hostname: "h".into(), ..Default::default() };
        assert!(!serde_json::to_string(&data).unwrap().contains("pinned"));
        let pinned = HostData { pinned: true, ..data };
        let json = serde_json::to_string(&pinned).unwrap();
        assert!(json.contains("\"pinned\":true"));
        assert_eq!(serde_json::from_str::<HostData>(&json).unwrap(), pinned);
    }
}
