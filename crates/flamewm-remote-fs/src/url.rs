//! URL model: `sftp://`, `fish://`, `ssh://`, plus `smb://` and `ftp://` (parsed, not yet served).
//!
//! Path rules: an empty path means the remote home directory. `/~/x` means `~/x` relative to
//! home (fish-style). Passwords in URLs are rejected so secrets never reach logs.

use crate::Error;
use crate::transport::ConnKey;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Sftp,
    Fish,
    Ssh,
    Smb,
    Ftp,
}

impl Protocol {
    #[must_use]
    pub const fn scheme(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Fish => "fish",
            Self::Ssh => "ssh",
            Self::Smb => "smb",
            Self::Ftp => "ftp",
        }
    }

    #[must_use]
    pub const fn is_ssh(self) -> bool {
        matches!(self, Self::Sftp | Self::Fish | Self::Ssh)
    }

    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Smb => 445,
            Self::Ftp => 21,
            _ => 22,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Url {
    pub protocol: Protocol,
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
}

fn invalid(why: &str) -> Error {
    Error::InvalidUrl(why.to_string())
}

impl Url {
    pub fn parse(input: &str) -> Result<Self, Error> {
        let (scheme, rest) = input
            .split_once("://")
            .ok_or_else(|| invalid("missing scheme"))?;
        let protocol = match scheme.to_ascii_lowercase().as_str() {
            "sftp" => Protocol::Sftp,
            "fish" => Protocol::Fish,
            "ssh" => Protocol::Ssh,
            "smb" => Protocol::Smb,
            "ftp" => Protocol::Ftp,
            _ => return Err(invalid("unknown scheme")),
        };
        let (authority, raw_path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        let (user, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, authority),
        };
        if user.is_some_and(|u| u.contains(':')) {
            return Err(invalid("passwords are not accepted in URLs"));
        }
        let (host, port) = parse_hostport(hostport)?;
        let user = match user {
            Some(u) => {
                let u = percent_decode(u)?;
                check_chars(&u, "user", |c| {
                    c.is_ascii_alphanumeric() || "._-+".contains(c)
                })?;
                Some(u)
            }
            None => None,
        };
        check_chars(&host, "host", |c| {
            c.is_ascii_alphanumeric() || ".-_:".contains(c)
        })?;
        let path = percent_decode(raw_path)?;
        Ok(Self {
            protocol,
            user,
            host,
            port,
            path,
        })
    }

    #[must_use]
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or_else(|| self.protocol.default_port())
    }

    #[must_use]
    pub fn conn_key(&self) -> ConnKey {
        ConnKey {
            user: self.user.clone(),
            host: self.host.clone(),
            port: self.port_or_default(),
        }
    }

    /// Path as the remote shell should see it: `~` for home, otherwise an absolute or relative path.
    #[must_use]
    pub fn remote_path(&self) -> String {
        if self.path.is_empty() {
            return "~".into();
        }
        match self.path.strip_prefix("/~") {
            Some("") => "~".into(),
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => self.path.clone(),
        }
    }

    /// Last path component for display; `~` for home.
    #[must_use]
    pub fn file_name(&self) -> &str {
        let trimmed = self.path.trim_end_matches('/');
        match trimmed {
            "" | "/~" => "~",
            _ => trimmed.rsplit('/').next().unwrap_or(trimmed),
        }
    }

    #[must_use]
    pub fn join(&self, name: &str) -> Self {
        let base = if self.path.is_empty() {
            "/~"
        } else {
            self.path.trim_end_matches('/')
        };
        let mut child = self.clone();
        child.path = format!("{base}/{name}");
        child
    }

    /// Parent directory, or `None` at root or home (no upward navigation past home is modelled).
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let p = self.path.trim_end_matches('/');
        if matches!(p, "" | "/" | "/~") {
            return None;
        }
        let idx = p.rfind('/')?;
        let mut parent = self.clone();
        parent.path = if idx == 0 {
            "/".to_string()
        } else {
            p[..idx].to_string()
        };
        Some(parent)
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://", self.protocol.scheme())?;
        if let Some(u) = &self.user {
            write!(f, "{u}@")?;
        }
        if self.host.contains(':') {
            write!(f, "[{}]", self.host)?;
        } else {
            write!(f, "{}", self.host)?;
        }
        write!(f, ":{}{}", self.port_or_default(), self.path)
    }
}

fn parse_hostport(s: &str) -> Result<(String, Option<u16>), Error> {
    let (host, port_str) = if let Some(inner) = s.strip_prefix('[') {
        let end = inner
            .find(']')
            .ok_or_else(|| invalid("unterminated IPv6 literal"))?;
        let after = &inner[end + 1..];
        let port = if after.is_empty() {
            None
        } else {
            Some(
                after
                    .strip_prefix(':')
                    .ok_or_else(|| invalid("bad host syntax"))?,
            )
        };
        (&inner[..end], port)
    } else {
        match s.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (s, None),
        }
    };
    let port = match port_str {
        None | Some("") => None,
        Some(p) => {
            let n: u16 = p.parse().map_err(|_| invalid("bad port"))?;
            if n == 0 {
                return Err(invalid("bad port"));
            }
            Some(n)
        }
    };
    if host.is_empty() {
        return Err(invalid("empty host"));
    }
    Ok((host.to_string(), port))
}

fn check_chars(s: &str, what: &str, ok: impl Fn(char) -> bool) -> Result<(), Error> {
    // A leading '-' would be parsed as an ssh option; reject it to rule out option injection.
    if s.is_empty() || s.starts_with('-') || !s.chars().all(ok) {
        return Err(Error::InvalidUrl(format!("invalid {what}")));
    }
    Ok(())
}

fn percent_decode(s: &str) -> Result<String, Error> {
    if !s.contains('%') {
        return Ok(s.to_string());
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| invalid("truncated percent escape"))?;
            let hex = std::str::from_utf8(hex).map_err(|_| invalid("bad percent escape"))?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| invalid("bad percent escape"))?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| invalid("path is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sftp_with_user_port_and_path() {
        let u = Url::parse("sftp://ubuntu@play.example.com:2222/srv/www").unwrap();
        assert_eq!(u.protocol, Protocol::Sftp);
        assert_eq!(u.user.as_deref(), Some("ubuntu"));
        assert_eq!(u.host, "play.example.com");
        assert_eq!(u.port_or_default(), 2222);
        assert_eq!(u.remote_path(), "/srv/www");
    }

    #[test]
    fn empty_path_is_home_and_fish_tilde_paths_map() {
        let home = Url::parse("fish://host").unwrap();
        assert_eq!(home.remote_path(), "~");
        assert_eq!(home.port_or_default(), 22);
        let t = Url::parse("fish://host/~/docs").unwrap();
        assert_eq!(t.remote_path(), "~/docs");
        assert_eq!(Url::parse("fish://host/~").unwrap().remote_path(), "~");
    }

    #[test]
    fn ssh_alias_and_ipv6_and_percent_escapes() {
        let u = Url::parse("ssh://[fe80::1]:2200/a%20b").unwrap();
        assert_eq!(u.protocol, Protocol::Ssh);
        assert_eq!(u.host, "fe80::1");
        assert_eq!(u.port, Some(2200));
        assert_eq!(u.path, "/a b");
        assert_eq!(u.to_string(), "ssh://[fe80::1]:2200/a b");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Url::parse("nonsense").is_err());
        assert!(Url::parse("gopher://h/").is_err());
        assert!(
            Url::parse("sftp://user:secret@h/").is_err(),
            "passwords in URL must be refused"
        );
        assert!(
            Url::parse("sftp://-oProxyCommand=evil/").is_err(),
            "option injection"
        );
        assert!(Url::parse("sftp://h:0/").is_err());
        assert!(Url::parse("sftp://h:99999/").is_err());
        assert!(Url::parse("sftp:///path").is_err());
        assert!(Url::parse("sftp://h/%zz").is_err());
        assert!(Url::parse("sftp://h;rm/").is_err());
    }

    #[test]
    fn join_parent_and_names() {
        let base = Url::parse("sftp://h/srv").unwrap();
        let child = base.join("www");
        assert_eq!(child.path, "/srv/www");
        assert_eq!(child.file_name(), "www");
        assert_eq!(child.parent().unwrap().path, "/srv");
        assert_eq!(
            Url::parse("sftp://h/srv").unwrap().parent().unwrap().path,
            "/"
        );
        assert!(Url::parse("sftp://h/").unwrap().parent().is_none());
        let home = Url::parse("fish://h").unwrap().join("a");
        assert_eq!(home.path, "/~/a");
        assert_eq!(home.remote_path(), "~/a");
        assert_eq!(home.parent().unwrap().remote_path(), "~");
        assert_eq!(Url::parse("fish://h").unwrap().file_name(), "~");
    }
}
