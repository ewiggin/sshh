//! Parser de la línea de comandos de OpenSSH.
//!
//! Solo extrae lo que sshh necesita (destino, usuario, puerto, identidades,
//! salto y opciones `-o`). Los argumentos originales se pasan a `ssh` intactos.

use std::fmt;

/// Flags de `ssh` que consumen un argumento.
const FLAGS_WITH_ARG: &str = "BbcDEeFIiJLlmOoPpQRSWw";
/// Flags de `ssh` sin argumento.
const FLAGS_NO_ARG: &str = "46AaCfGgKkMNnqsTtVvXxYy";

/// Destino de una conexión: `[user@]host` o `ssh://[user@]host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl Destination {
    pub fn parse(s: &str) -> Option<Self> {
        let (rest, is_uri) = match s.strip_prefix("ssh://") {
            Some(rest) => (rest.trim_end_matches('/'), true),
            None => (s, false),
        };
        let (user, hostport) = match rest.rsplit_once('@') {
            Some((u, h)) => ((!u.is_empty()).then(|| u.to_string()), h),
            None => (None, rest),
        };
        let (host, port) = if is_uri {
            split_host_port(hostport)?
        } else {
            (hostport, None)
        };
        if host.is_empty() || host.chars().any(char::is_whitespace) {
            return None;
        }
        Some(Self {
            user,
            host: host.to_string(),
            port,
        })
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.user, self.port) {
            (user, Some(port)) => {
                f.write_str("ssh://")?;
                if let Some(user) = user {
                    write!(f, "{user}@")?;
                }
                if self.host.contains(':') {
                    write!(f, "[{}]:{port}", self.host)
                } else {
                    write!(f, "{}:{port}", self.host)
                }
            }
            (Some(user), None) => write!(f, "{user}@{}", self.host),
            (None, None) => f.write_str(&self.host),
        }
    }
}

/// Separa `host[:port]` o `[ipv6][:port]` dentro de una URI ssh://.
fn split_host_port(s: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p.parse().ok()?),
            None if after.is_empty() => None,
            None => return None,
        };
        return Some((host, port));
    }
    match s.split_once(':') {
        Some((host, p)) => Some((host, Some(p.parse().ok()?))),
        None => Some((s, None)),
    }
}

/// Separa una opción `-o` en clave y valor (`Key=Value` o `Key Value`).
pub fn parse_option(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let split = s.find(|c: char| c == '=' || c.is_whitespace())?;
    let key = s[..split].trim();
    let value = s[split..]
        .trim_start_matches(|c: char| c.is_whitespace())
        .trim_start_matches('=')
        .trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    (!key.is_empty()).then(|| (key.to_string(), value.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    UnknownFlag(char),
    MissingValue(char),
    InvalidPort(String),
    InvalidDestination(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFlag(c) => write!(f, "flag desconocido -{c}"),
            Self::MissingValue(c) => write!(f, "falta el valor de -{c}"),
            Self::InvalidPort(p) => write!(f, "puerto inválido «{p}»"),
            Self::InvalidDestination(d) => write!(f, "destino inválido «{d}»"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Resultado de analizar los argumentos de una llamada a `ssh`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SshInvocation {
    /// Destino y su posición en la lista de argumentos.
    pub destination: Option<(usize, Destination)>,
    pub login: Option<String>,
    pub port: Option<u16>,
    pub identities: Vec<String>,
    pub jump: Option<String>,
    pub options: Vec<(String, String)>,
    /// La llamada no abre una sesión (`-G`, `-V`, `-O`, `-Q`): solo consulta.
    pub info_only: bool,
}

impl SshInvocation {
    pub fn option(&self, key: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn effective_user(&self) -> Option<&str> {
        self.destination
            .as_ref()
            .and_then(|(_, d)| d.user.as_deref())
            .or(self.login.as_deref())
            .or_else(|| self.option("User"))
    }

    pub fn effective_port(&self) -> Option<u16> {
        self.destination
            .as_ref()
            .and_then(|(_, d)| d.port)
            .or(self.port)
            .or_else(|| self.option("Port").and_then(|p| p.parse().ok()))
    }

    /// Indica si el usuario ya fijó la opción `key` en la línea de comandos.
    pub fn sets(&self, key: &str) -> bool {
        match key.to_ascii_lowercase().as_str() {
            "user" => self.effective_user().is_some(),
            "port" => self.effective_port().is_some(),
            "identityfile" => !self.identities.is_empty() || self.option(key).is_some(),
            "proxyjump" => self.jump.is_some() || self.option(key).is_some(),
            _ => self.option(key).is_some(),
        }
    }

    fn apply_flag(&mut self, flag: char, value: String) -> Result<(), ParseError> {
        match flag {
            'l' => self.login = Some(value),
            'p' => self.port = Some(value.parse().map_err(|_| ParseError::InvalidPort(value))?),
            'i' => self.identities.push(value),
            'J' => self.jump = Some(value),
            'o' => self.options.extend(parse_option(&value)),
            _ => {}
        }
        Ok(())
    }

    fn set_destination(&mut self, index: usize, arg: &str) -> Result<(), ParseError> {
        let dest =
            Destination::parse(arg).ok_or_else(|| ParseError::InvalidDestination(arg.into()))?;
        self.destination = Some((index, dest));
        Ok(())
    }
}

/// Analiza los argumentos igual que `ssh`: opciones, destino, más opciones y
/// después el comando remoto (a partir del primer argumento que no es opción).
pub fn parse(args: &[String]) -> Result<SshInvocation, ParseError> {
    let mut inv = SshInvocation::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            if inv.destination.is_none()
                && let Some(next) = args.get(i + 1)
            {
                inv.set_destination(i + 1, next)?;
            }
            break;
        }
        if let Some(flags) = arg.strip_prefix('-').filter(|f| !f.is_empty()) {
            for (pos, c) in flags.char_indices() {
                if matches!(c, 'G' | 'V' | 'O' | 'Q') {
                    inv.info_only = true;
                }
                if FLAGS_NO_ARG.contains(c) {
                    continue;
                }
                if !FLAGS_WITH_ARG.contains(c) {
                    return Err(ParseError::UnknownFlag(c));
                }
                let rest = &flags[pos + c.len_utf8()..];
                let value = if rest.is_empty() {
                    i += 1;
                    args.get(i).cloned().ok_or(ParseError::MissingValue(c))?
                } else {
                    rest.to_string()
                };
                inv.apply_flag(c, value)?;
                break;
            }
            i += 1;
            continue;
        }
        if inv.destination.is_some() {
            // Empieza el comando remoto.
            break;
        }
        inv.set_destination(i, arg)?;
        i += 1;
    }
    Ok(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn dest(inv: &SshInvocation) -> (usize, &Destination) {
        let (i, d) = inv.destination.as_ref().unwrap();
        (*i, d)
    }

    #[test]
    fn destination_forms() {
        let d = Destination::parse("root@example.com").unwrap();
        assert_eq!((d.user.as_deref(), d.host.as_str(), d.port), (Some("root"), "example.com", None));

        let d = Destination::parse("ssh://bob@[::1]:2222").unwrap();
        assert_eq!((d.user.as_deref(), d.host.as_str(), d.port), (Some("bob"), "::1", Some(2222)));

        let d = Destination::parse("ssh://host:22/").unwrap();
        assert_eq!((d.user, d.host.as_str(), d.port), (None, "host", Some(22)));

        assert!(Destination::parse("ssh://host:abc").is_none());
        assert!(Destination::parse("@").is_none());
    }

    #[test]
    fn simple_destination() {
        let inv = parse(&args("user@host")).unwrap();
        assert_eq!(dest(&inv).0, 0);
        assert_eq!(inv.effective_user(), Some("user"));
    }

    #[test]
    fn options_before_and_after_destination() {
        let inv = parse(&args("-v -p 2222 host -i key -J jump ls -la")).unwrap();
        assert_eq!(dest(&inv).0, 3);
        assert_eq!(inv.port, Some(2222));
        assert_eq!(inv.identities, vec!["key"]);
        assert_eq!(inv.jump.as_deref(), Some("jump"));
    }

    #[test]
    fn combined_flags_and_attached_values() {
        let inv = parse(&args("-vvAp2200 -lroot -oUser=x host")).unwrap();
        assert_eq!(inv.port, Some(2200));
        assert_eq!(inv.login.as_deref(), Some("root"));
        assert_eq!(inv.option("user"), Some("x"));
        assert_eq!(dest(&inv).0, 3);
    }

    #[test]
    fn remote_command_is_not_parsed() {
        let inv = parse(&args("host ls -p 1")).unwrap();
        assert_eq!(inv.port, None);
    }

    #[test]
    fn double_dash() {
        let inv = parse(&args("-p 22 -- host -p 3")).unwrap();
        assert_eq!(dest(&inv).0, 3);
        assert_eq!(inv.port, Some(22));
    }

    #[test]
    fn no_destination() {
        assert!(parse(&args("-V")).unwrap().destination.is_none());
        assert!(parse(&args("-Q cipher")).unwrap().destination.is_none());
    }

    #[test]
    fn info_only() {
        assert!(parse(&args("-G host")).unwrap().info_only);
        assert!(parse(&args("-O check host")).unwrap().info_only);
        assert!(!parse(&args("-v host")).unwrap().info_only);
    }

    #[test]
    fn errors() {
        assert_eq!(parse(&args("-Z host")), Err(ParseError::UnknownFlag('Z')));
        assert_eq!(parse(&args("host -p")), Err(ParseError::MissingValue('p')));
        assert!(matches!(parse(&args("-p x host")), Err(ParseError::InvalidPort(_))));
    }

    #[test]
    fn option_forms() {
        assert_eq!(parse_option("Port=22"), Some(("Port".into(), "22".into())));
        assert_eq!(parse_option("Port 22"), Some(("Port".into(), "22".into())));
        assert_eq!(parse_option("Port = 22"), Some(("Port".into(), "22".into())));
        assert_eq!(
            parse_option("IdentityFile=\"~/my key\""),
            Some(("IdentityFile".into(), "~/my key".into()))
        );
        assert_eq!(parse_option("Port"), None);
    }

    #[test]
    fn precedence_of_user_and_port() {
        let inv = parse(&args("-l a -o User=b ssh://c@h:1")).unwrap();
        assert_eq!(inv.effective_user(), Some("c"));
        assert_eq!(inv.effective_port(), Some(1));
        assert!(inv.sets("USER"));
        assert!(!inv.sets("ProxyJump"));
    }
}
