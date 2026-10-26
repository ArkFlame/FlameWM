//! Directory entries and parsers for remote `ls` output.
//!
//! Two dialects are accepted:
//! - `ls -lAn --time-style=+%s` (what the client sends; numeric owner/group, epoch mtime).
//! - `ls -l` longname style used by sftp servers (`Mon DD HH:MM` / `Mon DD YYYY`).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    /// Unix seconds; 0 when unknown.
    pub mtime: i64,
    /// Permission bits including setuid/setgid/sticky (`0o7777` range).
    pub mode: u32,
    pub link_target: Option<String>,
}

/// Split the next whitespace-delimited token. The returned remainder keeps its leading space.
fn next_field(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start_matches(' ');
    if s.is_empty() {
        return None;
    }
    match s.find(' ') {
        Some(i) => Some((&s[..i], &s[i..])),
        None => Some((s, "")),
    }
}

fn parse_mode(s: &str) -> Option<(Kind, u32)> {
    let b = s.as_bytes();
    if b.len() != 10 {
        return None;
    }
    let kind = match b[0] {
        b'-' => Kind::File,
        b'd' => Kind::Dir,
        b'l' => Kind::Symlink,
        _ => Kind::Other,
    };
    let mut mode = 0u32;
    for (idx, bit) in [
        (1, 0o400),
        (2, 0o200),
        (4, 0o40),
        (5, 0o20),
        (7, 0o4),
        (8, 0o2),
    ] {
        if b[idx] != b'-' {
            mode |= bit;
        }
    }
    // Execute slots: `x` plain; `s`/`t` imply execute; `S`/`T` mean no execute.
    for (idx, bit) in [(3, 0o100), (6, 0o10), (9, 0o1)] {
        if matches!(b[idx], b'x' | b's' | b't') {
            mode |= bit;
        }
    }
    if matches!(b[3], b's' | b'S') {
        mode |= 0o4000;
    }
    if matches!(b[6], b's' | b'S') {
        mode |= 0o2000;
    }
    if matches!(b[9], b't' | b'T') {
        mode |= 0o1000;
    }
    Some((kind, mode))
}

fn build(mode_str: &str, size: u64, mtime: i64, name_raw: &str) -> Option<Entry> {
    let (kind, mode) = parse_mode(mode_str)?;
    let (name, link_target) = match (kind, name_raw.split_once(" -> ")) {
        (Kind::Symlink, Some((n, t))) => (n, Some(t.to_string())),
        _ => (name_raw, None),
    };
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(Entry {
        name: name.to_string(),
        kind,
        size,
        mtime,
        mode,
        link_target,
    })
}

/// Parse one line of `ls -lAn --time-style=+%s`.
#[must_use]
pub fn parse_ls_numeric(line: &str) -> Option<Entry> {
    let (mode, r) = next_field(line)?;
    let (_links, r) = next_field(r)?;
    let (_uid, r) = next_field(r)?;
    let (_gid, r) = next_field(r)?;
    let (size, r) = next_field(r)?;
    let (time, r) = next_field(r)?;
    let name = r.strip_prefix(' ')?;
    build(mode, size.parse().ok()?, time.parse().ok()?, name)
}

/// Parse one sftp-longname line. `now_year` resolves `Mon DD HH:MM` timestamps (UTC).
#[must_use]
pub fn parse_longname(line: &str, now_year: i64) -> Option<Entry> {
    let (mode, r) = next_field(line)?;
    let (_links, r) = next_field(r)?;
    let (_owner, r) = next_field(r)?;
    let (_group, r) = next_field(r)?;
    let (size, r) = next_field(r)?;
    let (month, r) = next_field(r)?;
    let (day, r) = next_field(r)?;
    let (when, r) = next_field(r)?;
    let name = r.strip_prefix(' ')?;
    let month = MONTHS.iter().position(|m| *m == month)? as i64 + 1;
    let day: i64 = day.parse().ok()?;
    let mtime = if let Some((hh, mm)) = when.split_once(':') {
        civil_to_epoch(now_year, month, day, hh.parse().ok()?, mm.parse().ok()?)
    } else {
        civil_to_epoch(when.parse().ok()?, month, day, 0, 0)
    };
    build(mode, size.parse().ok()?, mtime, name)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Days-from-civil (Howard Hinnant), UTC.
fn civil_to_epoch(year: i64, month: i64, day: i64, hour: i64, minute: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hour * 3600 + minute * 60
}

/// Parse a whole listing, skipping `total` headers and unparseable lines.
#[must_use]
pub fn parse_listing(output: &str, longname_year: Option<i64>) -> Vec<Entry> {
    output
        .lines()
        .filter(|l| !l.starts_with("total "))
        .filter_map(|l| match longname_year {
            Some(y) => parse_longname(l, y),
            None => parse_ls_numeric(l),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numeric_ls_file_dir_and_symlink() {
        let f = parse_ls_numeric("-rw-r--r-- 1 1000 1000 1234 1700000000 notes with spaces.txt")
            .unwrap();
        assert_eq!(f.kind, Kind::File);
        assert_eq!(f.size, 1234);
        assert_eq!(f.mtime, 1_700_000_000);
        assert_eq!(f.mode, 0o644);
        assert_eq!(f.name, "notes with spaces.txt");

        let d = parse_ls_numeric("drwxr-x--- 2 0 0 4096 1700000001 .config").unwrap();
        assert_eq!(d.kind, Kind::Dir);
        assert_eq!(d.mode, 0o750);
        assert_eq!(d.name, ".config");

        let l = parse_ls_numeric("lrwxrwxrwx 1 0 0 7 1700000002 current -> releases/v2").unwrap();
        assert_eq!(l.kind, Kind::Symlink);
        assert_eq!(l.name, "current");
        assert_eq!(l.link_target.as_deref(), Some("releases/v2"));
    }

    #[test]
    fn special_bits_and_padding() {
        let e = parse_ls_numeric("-rwsr-xr-t   1     0     0          0 1700000000 bin").unwrap();
        assert_eq!(e.mode, 0o4000 | 0o1000 | 0o755);
    }

    #[test]
    fn parses_sftp_longname_dialect() {
        let e = parse_longname(
            "-rw-r--r--    1 1000     1000       2048 Oct 22 10:30 readme.md",
            2026,
        )
        .unwrap();
        assert_eq!(e.size, 2048);
        assert_eq!(e.name, "readme.md");
        assert_eq!(e.mtime, civil_to_epoch(2026, 10, 22, 10, 30));
        let old = parse_longname(
            "drwxr-xr-x    2 root     root         4096 Mar  3  2019 old",
            2026,
        )
        .unwrap();
        assert_eq!(old.kind, Kind::Dir);
        assert_eq!(old.mtime, civil_to_epoch(2019, 3, 3, 0, 0));
    }

    #[test]
    fn listing_skips_total_and_dots() {
        let out = "total 8\n. stuff\n-rw-r--r-- 1 0 0 1 1 a\ngarbage\n";
        let v = parse_listing(out, None);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "a");
    }

    #[test]
    fn epoch_known_value() {
        // 2000-01-01T00:00:00Z
        assert_eq!(civil_to_epoch(2000, 1, 1, 0, 0), 946_684_800);
    }
}
