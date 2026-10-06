use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Opción adicional de ssh_config (`Key Value`) asociada a una conexión.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshOption {
    pub key: String,
    pub value: String,
}

/// Datos editables de una conexión. Es también el formato de import/export.
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
}

impl HostData {
    pub fn validate(&self) -> Result<()> {
        validate_alias(&self.alias)?;
        if self.hostname.is_empty() || self.hostname.chars().any(char::is_whitespace) {
            bail!("hostname inválido «{}»", self.hostname);
        }
        if self.port == Some(0) {
            bail!("el puerto no puede ser 0");
        }
        Ok(())
    }

    /// Destino legible: `user@hostname:port`.
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

    /// Opciones de ssh_config que describen la conexión, en orden.
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

/// Opciones de ssh_config cuyo valor es un único argumento (rutas, nombres).
/// Las demás pueden tener varios (`LocalForward 8080 localhost:80`) y no se
/// deben entrecomillar.
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

/// Valor de una opción tal como debe escribirse en ssh_config o en `-o`.
pub fn config_value(key: &str, value: &str) -> String {
    let single = SINGLE_VALUE_OPTIONS.contains(&key.to_ascii_lowercase().as_str());
    if single && value.chars().any(char::is_whitespace) && !value.starts_with('"') {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

/// Conexión guardada en la base de datos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: i64,
    pub data: HostData,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used: Option<i64>,
    pub use_count: i64,
}

/// Subcomandos propios de sshh; no pueden usarse como alias.
pub const RESERVED_ALIASES: &[&str] =
    &["ls", "add", "rm", "help", "history", "export", "import", "import-ssh-config", "ssh-config"];

pub fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty() {
        bail!("el alias no puede estar vacío");
    }
    if alias.starts_with('-') {
        bail!("el alias no puede empezar por «-»");
    }
    if let Some(c) = alias
        .chars()
        .find(|c| c.is_whitespace() || matches!(c, '*' | '?' | '!' | ',' | '@' | '"' | '#'))
    {
        bail!("el alias no puede contener «{c}»");
    }
    if RESERVED_ALIASES.contains(&alias) {
        bail!("«{alias}» es un subcomando de sshh");
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
}
