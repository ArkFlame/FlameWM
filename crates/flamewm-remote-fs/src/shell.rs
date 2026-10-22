//! Remote shell command builders. Every user-controlled path goes through `path_arg`.
//! Remote hosts are assumed POSIX sh with GNU coreutils (Linux). Exit code 17 means "exists".

/// Single-quote for POSIX sh.
#[must_use]
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Shell word for a remote path. `~` and `~/x` expand against `$HOME` on the remote side.
#[must_use]
pub fn path_arg(path: &str) -> String {
    if path == "~" {
        return "\"$HOME\"".into();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("\"$HOME\"/{}", quote(rest));
    }
    quote(path)
}

pub const HOME: &str = "printf '%s\\n' \"$HOME\"";

#[must_use]
pub fn list(path: &str) -> String {
    format!("LC_ALL=C ls -lAn --time-style=+%s -- {}", path_arg(path))
}

/// Record separator that ends the symlink prefix of [`list_with_links`]. Its output is `name\0...` then `\x1e\n`.
pub const LINK_MARK: &str = "\x1e";

/// One round trip: NUL-separated names of symlinks that point at a directory, then [`LINK_MARK`] and a newline,
/// then the normal `ls` listing. `find` runs first, so the prefix is complete before any entry row arrives.
#[must_use]
pub fn list_with_links(path: &str) -> String {
    let p = path_arg(path);
    format!(
        "find {p} -mindepth 1 -maxdepth 1 -type l -exec test -d {{}} \\; -printf '%f\\0' 2>/dev/null; \
         printf '\\036\\n'; LC_ALL=C ls -lAn --time-style=+%s -- {p}"
    )
}

#[must_use]
pub fn stat(path: &str) -> String {
    format!("LC_ALL=C ls -lAnd --time-style=+%s -- {}", path_arg(path))
}

#[must_use]
pub fn mkdir(path: &str) -> String {
    format!("mkdir -- {}", path_arg(path))
}

#[must_use]
pub fn remove(path: &str, recursive: bool) -> String {
    format!("rm {} -- {}", if recursive { "-rf" } else { "-f" }, path_arg(path))
}

#[must_use]
pub fn rename(from: &str, to: &str) -> String {
    let (f, t) = (path_arg(from), path_arg(to));
    format!("if [ -e {t} ] || [ -L {t} ]; then exit 17; fi; mv -- {f} {t}")
}

#[must_use]
pub fn cat(path: &str) -> String {
    format!("cat -- {}", path_arg(path))
}

/// Write stdin to `tmp`, then atomically move it over `dst`.
#[must_use]
pub fn write_atomic(tmp: &str, dst: &str) -> String {
    let (t, d) = (path_arg(tmp), path_arg(dst));
    format!("cat > {t} && mv -f -- {t} {d}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_apostrophes_and_spaces() {
        assert_eq!(quote("it's a dir"), "'it'\\''s a dir'");
        assert_eq!(path_arg("~"), "\"$HOME\"");
        assert_eq!(path_arg("~/a b"), "\"$HOME\"/'a b'");
        assert_eq!(cat("/x;rm -rf /"), "cat -- '/x;rm -rf /'");
    }
}
