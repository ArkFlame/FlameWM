//! Inline command prediction (zsh-autosuggestions style). Every terminal window shares one "most used" store file;
//! a trie keeps, for each prefix, the best full command in its subtree (most uses, ties to the most recent), so a
//! lookup walks the prefix only. Other windows' new lines are read from the file's offset when the user types, at
//! most once a second. The typed line is tracked from the keys the child receives (`LineInput`).
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};

use crate::keys::Mods;

const MAX_COMMAND_BYTES: usize = 4096;
/// Shell history read at first run: only the last bytes, so a huge file costs little.
const SEED_CAP_BYTES: u64 = 2 << 20;
/// The store is rewritten with aggregated counts once it grows past this.
const COMPACT_BYTES: u64 = 1 << 20;
const HEAD_BYTES: usize = 64;
const REFRESH_EVERY: Duration = Duration::from_secs(1);
/// Commands containing these (lowercase) are never stored.
const SECRET_MARKERS: [&str; 14] = [
    "password",
    "passwd",
    "passphrase",
    "secret=",
    "token=",
    "api_key",
    "apikey",
    "api-key",
    "access_key",
    "private_key",
    "authorization",
    "bearer ",
    "credential",
    "--pass",
];
/// Words that mark a variable name, long option or `-u user:pass` value as a credential.
const SECRET_WORDS: [&str; 7] = ["pass", "pwd", "secret", "token", "key", "auth", "credential"];

/// Ordering of a command: uses first, then recency (the sequence number of its last line).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    count: u64,
    seq: u64,
}

/// Most commands the trie keeps. Past twice this, `Trie::add` rebuilds it from the highest-ranked half.
const MAX_COMMANDS: usize = 4000;

#[derive(Default)]
struct Node {
    /// A short list: most prefixes have one or two children, so a map per node costs far more than its entries.
    children: Vec<(char, usize)>,
    /// Best command strictly below this prefix (a command equal to the prefix is not a suggestion for it).
    best: Option<(Rank, usize)>,
}

struct Command {
    text: String,
    rank: Rank,
}

/// Prefix trie over commands. Each node caches the best strict extension, so suggestions cost O(prefix length).
pub struct Trie {
    nodes: Vec<Node>,
    commands: Vec<Command>,
    ids: HashMap<String, usize>,
}

impl Default for Trie {
    fn default() -> Self {
        Self { nodes: vec![Node::default()], commands: Vec::new(), ids: HashMap::new() }
    }
}

impl Trie {
    /// Adds `uses` to `text` and marks it as used at sequence `seq` (later than every earlier use).
    pub fn add(&mut self, text: &str, uses: u64, seq: u64) {
        let id = match self.ids.get(text) {
            Some(&id) => {
                let command = &mut self.commands[id];
                command.rank = Rank { count: command.rank.count + uses, seq };
                id
            }
            None => {
                let id = self.commands.len();
                self.commands.push(Command { text: text.to_owned(), rank: Rank { count: uses, seq } });
                self.ids.insert(text.to_owned(), id);
                id
            }
        };
        let rank = self.commands[id].rank;
        let mut node = 0;
        for c in text.chars() {
            // Only ranks grow, so this command replaces the cached best unless another command still outranks it.
            let slot = &mut self.nodes[node].best;
            if slot.is_none_or(|(best, best_id)| best_id == id || rank > best) {
                *slot = Some((rank, id));
            }
            node = match self.child(node, c) {
                Some(next) => next,
                None => {
                    self.nodes.push(Node::default());
                    let next = self.nodes.len() - 1;
                    self.nodes[node].children.push((c, next));
                    next
                }
            };
        }
        if self.commands.len() > 2 * MAX_COMMANDS {
            self.prune();
        }
    }

    fn child(&self, node: usize, c: char) -> Option<usize> {
        self.nodes[node].children.iter().find(|(ch, _)| *ch == c).map(|&(_, next)| next)
    }

    /// Rebuilds the trie from the `MAX_COMMANDS` highest-ranked commands, so the store stays bounded.
    fn prune(&mut self) {
        let mut kept: Vec<(Rank, String)> = self.commands.drain(..).map(|command| (command.rank, command.text)).collect();
        kept.sort_by(|a, b| b.0.cmp(&a.0));
        kept.truncate(MAX_COMMANDS);
        *self = Self::default();
        for (rank, text) in kept {
            self.add(&text, rank.count, rank.seq);
        }
    }

    /// The best stored command that starts with `prefix` and is longer than it.
    pub fn best_extension(&self, prefix: &str) -> Option<&str> {
        let mut node = 0;
        for c in prefix.chars() {
            node = self.child(node, c)?;
        }
        let (_, id) = self.nodes[node].best?;
        Some(&self.commands[id].text)
    }

    pub fn uses(&self, text: &str) -> Option<u64> {
        self.ids.get(text).map(|&id| self.commands[id].rank.count)
    }
}

/// Whether a command may be stored: not blank, not space-prefixed (HISTCONTROL=ignorespace), short, single line,
/// and free of obvious secret markers.
pub fn admissible(command: &str) -> bool {
    if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES || command.starts_with([' ', '\t']) {
        return false;
    }
    if command.contains(['\n', '\r']) {
        return false;
    }
    !secret_bearing(command)
}

/// Whether a command line carries a credential: secret words in the text, `scheme://user:pass@host`, a secret
/// assignment (`export TOKEN=x`, `KEY=x cmd`), a secret-named long option, `-p<pass>`, `-u user:pass`, or `sudo -S`.
pub fn secret_bearing(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    if SECRET_MARKERS.iter().any(|marker| lower.contains(marker)) || url_with_credentials(&lower) {
        return true;
    }
    let has_sudo = lower.split_whitespace().any(|word| word == "sudo");
    let mut previous = "";
    for word in lower.split_whitespace() {
        let word = word.trim_matches(|c| c == '"' || c == '\'');
        let secret_name = |name: &str| SECRET_WORDS.iter().any(|secret| name.contains(secret));
        if let Some((name, _)) = word.split_once('=') {
            if !name.is_empty() && secret_name(name.trim_start_matches("--")) {
                return true;
            }
        }
        if let Some(long) = word.strip_prefix("--") {
            if secret_name(long.split('=').next().unwrap_or_default()) {
                return true;
            }
        }
        if word.len() > 2 && word.starts_with("-p") && !word.starts_with("--") {
            return true;
        }
        if has_sudo && word.starts_with('-') && !word.starts_with("--") && word.contains('s') {
            return true;
        }
        if matches!(previous, "-u" | "--user") && word.contains(':') {
            return true;
        }
        previous = word;
    }
    false
}

/// `scheme://user:password@host` in any word of an already lowercased line.
fn url_with_credentials(lower: &str) -> bool {
    lower.match_indices("://").any(|(at, _)| {
        let rest = &lower[at + 3..];
        let authority = rest.split(['/', ' ', '\t', '"', '\'']).next().unwrap_or_default();
        authority.split_once('@').is_some_and(|(userinfo, _)| userinfo.contains(':'))
    })
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t").replace('\r', "\\r")
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Folds store lines (`count<TAB>escaped command`) into one line per command, counts summed and ordered by last
/// use, so file order keeps meaning recency. Lines that fail the filter or do not parse are dropped.
fn compact_lines(text: &str) -> String {
    let mut totals: HashMap<String, (u64, usize)> = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let Some((count, escaped)) = line.split_once('\t') else { continue };
        let Ok(uses) = count.parse::<u64>() else { continue };
        let command = unescape(escaped);
        if uses == 0 || !admissible(&command) {
            continue;
        }
        let entry = totals.entry(command).or_insert((0, index));
        entry.0 += uses;
        entry.1 = index;
    }
    let mut ordered: Vec<(usize, u64, String)> = totals.into_iter().map(|(text, (uses, last))| (last, uses, text)).collect();
    ordered.sort_unstable_by_key(|entry| entry.0);
    let mut out = String::new();
    for (_, uses, command) in ordered {
        out.push_str(&format!("{uses}\t{}\n", escape(&command)));
    }
    out
}

/// Reads the last `SEED_CAP_BYTES` of a shell history file and returns store lines (one use each). zsh extended
/// entries (`: time:dur;command`), plain zsh and bash lines are accepted; multi-line entries and bash timestamps are
/// skipped.
fn shell_history_lines(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else { return String::new() };
    let Ok(meta) = file.metadata() else { return String::new() };
    let start = meta.len().saturating_sub(SEED_CAP_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    // A cut in the middle of the file starts with a partial line; drop it.
    let body = if start > 0 { text.split_once('\n').map_or("", |(_, rest)| rest) } else { &text };
    let mut out = String::new();
    let mut continued = false;
    for line in body.lines() {
        let was_continued = std::mem::replace(&mut continued, line.ends_with('\\'));
        if was_continued || continued || line.starts_with('#') {
            continue;
        }
        let command = match line.strip_prefix(": ").and_then(|rest| rest.split_once(';')) {
            Some((_, command)) => command,
            None => line,
        };
        if admissible(command) {
            out.push_str(&format!("1\t{}\n", escape(command)));
        }
    }
    out
}

/// Replaces `path` with `body` through a temporary file in the same directory.
fn atomic_write(path: &Path, body: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)
}

/// Appends one store line under an exclusive lock. If a compaction replaced the file while waiting for the lock,
/// the write retries on the new file.
fn append(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    for _ in 0..5 {
        let mut file = OpenOptions::new().append(true).create(true).open(path)?;
        flock(file.as_fd(), FlockOperation::LockExclusive)?;
        let same_file = match (fs::metadata(path), file.metadata()) {
            (Ok(current), Ok(open)) => current.dev() == open.dev() && current.ino() == open.ino(),
            _ => false,
        };
        if same_file {
            return file.write_all(line.as_bytes());
        }
    }
    Err(io::Error::other("terminal history file kept changing"))
}

/// Rewrites the store with aggregated counts while holding its lock, so no append is lost.
fn compact(path: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    flock(file.as_fd(), FlockOperation::LockExclusive)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    atomic_write(path, &compact_lines(&String::from_utf8_lossy(&bytes)))
}

/// Rewrites the store without lines that the filter now rejects (entries stored before a rule existed, or by
/// another program). Nothing is rewritten when every line passes.
fn purge_filtered(path: &Path) -> io::Result<()> {
    let text = String::from_utf8_lossy(&fs::read(path)?).into_owned();
    let stored_secret = text.lines().any(|line| {
        line.split_once('\t').is_some_and(|(count, escaped)| {
            count.parse::<u64>().is_ok_and(|uses| uses > 0) && !admissible(&unescape(escaped))
        })
    });
    if stored_secret { compact(path) } else { Ok(()) }
}

/// The store locations: `$XDG_DATA_HOME/flamewm/terminal-history` and the user's shell history files.
pub fn default_locations() -> (Option<PathBuf>, Vec<PathBuf>) {
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    let store = data.map(|data| data.join("flamewm/terminal-history"));
    let seeds = home.map(|home| vec![home.join(".zsh_history"), home.join(".bash_history")]).unwrap_or_default();
    (store, seeds)
}

/// The global "most used" command store as seen by this window.
pub struct History {
    path: Option<PathBuf>,
    trie: Trie,
    /// Bytes of the store already applied to `trie`.
    offset: u64,
    /// Device and inode of the file the offset refers to; a new inode (compaction) means a full reload.
    file: Option<(u64, u64)>,
    /// First bytes of the file as last read. Inode numbers are reused, so a replaced file is also caught by content.
    head: Vec<u8>,
    seq: u64,
    checked: Option<Instant>,
    /// Store size above which `record` compacts. Compaction folds duplicates only, so a store of mostly distinct
    /// commands stays large; the trigger then moves to twice its size instead of rewriting it on every record.
    compact_at: u64,
}

impl History {
    /// Opens the store. When it does not exist yet, it is created from the shell history files.
    pub fn open(path: Option<PathBuf>, seeds: &[PathBuf]) -> Self {
        if let Some(path) = &path {
            if !path.exists() {
                let body = compact_lines(&seeds.iter().map(|seed| shell_history_lines(seed)).collect::<String>());
                let _ = atomic_write(path, &body);
            } else {
                let _ = purge_filtered(path);
            }
        }
        let mut history = Self {
            path,
            trie: Trie::default(),
            offset: 0,
            file: None,
            head: Vec::new(),
            seq: 0,
            checked: None,
            compact_at: COMPACT_BYTES,
        };
        let _ = history.sync();
        history
    }

    pub fn open_default() -> Self {
        let (path, seeds) = default_locations();
        Self::open(path, &seeds)
    }

    /// The best suffix that completes `prefix` from the store. Empty or absent when nothing extends it.
    pub fn suggest(&self, prefix: &str) -> Option<&str> {
        self.trie.best_extension(prefix).and_then(|command| command.strip_prefix(prefix)).filter(|suffix| !suffix.is_empty())
    }

    /// Picks up other windows' new lines, at most once a second. Called on key presses, never per frame.
    pub fn refresh(&mut self) {
        if self.checked.is_none_or(|at| at.elapsed() >= REFRESH_EVERY) {
            let _ = self.sync();
        }
    }

    /// Applies the store lines past `offset`. A complete line only: a partly written last line waits for the next sync.
    pub fn sync(&mut self) -> io::Result<()> {
        self.checked = Some(Instant::now());
        let Some(path) = self.path.clone() else { return Ok(()) };
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let meta = file.metadata()?;
        let ident = (meta.dev(), meta.ino());
        let same = self.file == Some(ident) && meta.len() >= self.offset && self.head_matches(&mut file)?;
        if !same {
            self.trie = Trie::default();
            self.offset = 0;
            self.seq = 0;
            self.head.clear();
            self.file = Some(ident);
        }
        if meta.len() == self.offset {
            return Ok(());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if self.head.is_empty() {
            self.head = bytes[..bytes.len().min(HEAD_BYTES)].to_vec();
        }
        let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else { return Ok(()) };
        for line in bytes[..end].split(|&b| b == b'\n') {
            self.apply(&String::from_utf8_lossy(line));
        }
        self.offset += end as u64 + 1;
        Ok(())
    }

    fn head_matches(&self, file: &mut File) -> io::Result<bool> {
        if self.head.is_empty() {
            return Ok(true);
        }
        file.seek(SeekFrom::Start(0))?;
        let mut head = vec![0; self.head.len()];
        Ok(file.read_exact(&mut head).is_ok() && head == self.head)
    }

    fn apply(&mut self, line: &str) {
        let Some((count, escaped)) = line.split_once('\t') else { return };
        let Ok(uses) = count.parse::<u64>() else { return };
        let command = unescape(escaped);
        if uses == 0 || !admissible(&command) {
            return;
        }
        self.seq += 1;
        self.trie.add(&command, uses, self.seq);
    }

    /// Counts one use of `command` in the store and in this window. Filtered commands are not stored.
    pub fn record(&mut self, command: &str) -> io::Result<()> {
        let Some(path) = self.path.clone() else { return Ok(()) };
        if !admissible(command) {
            return Ok(());
        }
        append(&path, &format!("1\t{}\n", escape(command)))?;
        self.sync()?;
        if fs::metadata(&path).is_ok_and(|meta| meta.len() > self.compact_at) {
            compact(&path)?;
            let after = fs::metadata(&path).map_or(COMPACT_BYTES, |meta| meta.len());
            self.compact_at = (after * 2).max(COMPACT_BYTES);
            self.sync()?;
        }
        Ok(())
    }

    pub fn uses(&self, command: &str) -> Option<u64> {
        self.trie.uses(command)
    }
}

/// The line the user is typing, as the child sees it. It is `known` only while every key since the last Enter or
/// Ctrl+C was one this model follows exactly; anything else (history recall, completion, word kills) makes it unknown
/// until the next line starts.
#[derive(Debug)]
pub struct LineInput {
    chars: Vec<char>,
    cursor: usize,
    known: bool,
}

impl Default for LineInput {
    fn default() -> Self {
        Self { chars: Vec::new(), cursor: 0, known: true }
    }
}

fn printable(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

impl LineInput {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// True when a suggestion may be drawn: the line is exact and the cursor sits at its end.
    pub fn is_ready(&self) -> bool {
        self.known && !self.chars.is_empty() && self.cursor == self.chars.len()
    }

    /// Text inserted by the terminal itself (an accepted suggestion) or by a paste.
    pub fn insert_str(&mut self, text: &str) {
        for c in text.chars() {
            self.chars.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    pub fn paste(&mut self, text: &str) {
        if printable(text) {
            self.insert_str(text);
        } else {
            self.unknown();
        }
    }

    /// Applies one key press sent to the child. Returns the line on Enter when the line was exact.
    pub fn key(&mut self, name: &str, mods: Mods, text: Option<&str>) -> Option<String> {
        if mods.alt || (mods.shift && text.is_none()) {
            self.unknown();
            return None;
        }
        if mods.ctrl {
            match name {
                "a" => self.cursor = 0,
                "e" => self.cursor = self.chars.len(),
                "b" => self.cursor = self.cursor.saturating_sub(1),
                "f" => self.cursor = (self.cursor + 1).min(self.chars.len()),
                "h" => self.backspace(),
                "u" => {
                    self.chars.drain(..self.cursor);
                    self.cursor = 0;
                }
                "k" => self.chars.truncate(self.cursor),
                "c" => self.clear(),
                "l" => {}
                _ => self.unknown(),
            }
            return None;
        }
        match name {
            "return" | "enter" | "kp_enter" => {
                let submitted = (self.known && !self.chars.is_empty()).then(|| self.text());
                self.clear();
                return submitted;
            }
            "backspace" => self.backspace(),
            "delete" => {
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                } else {
                    self.unknown();
                }
            }
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => self.cursor = (self.cursor + 1).min(self.chars.len()),
            "home" => self.cursor = 0,
            "end" => self.cursor = self.chars.len(),
            _ => match text.filter(|t| printable(t)) {
                Some(text) => self.insert_str(text),
                None => self.unknown(),
            },
        }
        None
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        } else {
            self.unknown();
        }
    }

    fn unknown(&mut self) {
        self.known = false;
    }

    fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.known = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = PathBuf::from("/tmp").join(format!("flamewm-suggest-{name}-{}-{}", std::process::id(), nanos()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn nanos() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    fn ctrl() -> Mods {
        Mods { ctrl: true, ..Mods::default() }
    }

    fn type_text(line: &mut LineInput, text: &str) {
        for c in text.chars() {
            let s = c.to_string();
            assert_eq!(line.key(&s, Mods::default(), Some(&s)), None);
        }
    }

    #[test]
    fn trie_suggests_the_most_used_extension() {
        let mut trie = Trie::default();
        trie.add("git commit -m \"test\"", 1, 1);
        trie.add("git status", 3, 2);
        trie.add("git stash", 3, 3);
        // Equal uses: the more recent wins (seq 3 over seq 2).
        assert_eq!(trie.best_extension("g"), Some("git stash"));
        trie.add("git commit -m \"test\"", 5, 4);
        assert_eq!(trie.best_extension("g"), Some("git commit -m \"test\""));
        assert_eq!(trie.best_extension("git s"), Some("git stash"));
        assert_eq!(trie.best_extension("x"), None);
    }

    #[test]
    fn trie_never_suggests_the_prefix_itself() {
        let mut trie = Trie::default();
        trie.add("ls", 10, 1);
        assert_eq!(trie.best_extension("ls"), None, "an exact match is not an extension");
        trie.add("ls -la", 1, 2);
        assert_eq!(trie.best_extension("ls"), Some("ls -la"));
    }

    /// Past the cap the trie keeps the highest-ranked commands and still answers for them.
    #[test]
    fn trie_is_bounded_and_keeps_the_most_used_commands() {
        let mut trie = Trie::default();
        trie.add("git status", 500, 1);
        for i in 0..(2 * MAX_COMMANDS + 10) {
            trie.add(&format!("cmd{i:05}"), 1, 2 + i as u64);
        }
        assert!(trie.commands.len() <= 2 * MAX_COMMANDS, "{} commands kept", trie.commands.len());
        assert_eq!(trie.best_extension("git"), Some("git status"));
        assert_eq!(trie.uses("git status"), Some(500));
    }

    #[test]
    fn trie_ranking_follows_later_growth_of_a_lower_command() {
        let mut trie = Trie::default();
        trie.add("cargo build", 4, 1);
        trie.add("cargo test", 2, 2);
        assert_eq!(trie.best_extension("cargo "), Some("cargo build"));
        trie.add("cargo test", 3, 3);
        assert_eq!(trie.best_extension("cargo "), Some("cargo test"), "5 uses beat 4");
        assert_eq!(trie.uses("cargo test"), Some(5));
    }

    #[test]
    fn prefix_matching_is_by_character() {
        let mut trie = Trie::default();
        trie.add("echo héllo", 1, 1);
        assert_eq!(trie.best_extension("echo h"), Some("echo héllo"));
        assert_eq!(trie.best_extension("echo é"), None);
    }

    #[test]
    fn admissible_filters_secrets_and_leading_space() {
        assert!(admissible("git commit -m \"test\""));
        assert!(!admissible(" git status"), "space-prefixed commands are private");
        assert!(!admissible("   "));
        assert!(!admissible("mysql -p password=hunter2"));
        assert!(!admissible("curl -H X TOKEN=abc"));
        assert!(!admissible("curl -H 'Authorization: Bearer x'"));
        assert!(!admissible("line one\nline two"));
        assert!(!admissible(&"x".repeat(MAX_COMMAND_BYTES + 1)));
    }

    #[test]
    fn escaping_round_trips() {
        for text in ["a\\b", "tab\there", "back\\n literal", "plain"] {
            assert_eq!(unescape(&escape(text)), text);
        }
        assert_eq!(escape("a\nb"), "a\\nb");
    }

    #[test]
    fn compact_sums_counts_and_keeps_last_use_order() {
        let text = "1\tgit status\n1\tgit commit\n2\tgit status\n1\tgit commit\n0\tbroken\nnot a line\n";
        // `git status` last used at line 2, `git commit` at line 3: status is older, so it comes first.
        assert_eq!(compact_lines(text), "3\tgit status\n2\tgit commit\n");
    }

    #[test]
    fn store_merges_uses_from_two_windows() {
        let dir = tmp_dir("merge");
        let path = dir.join("terminal-history");
        let mut first = History::open(Some(path.clone()), &[]);
        let mut second = History::open(Some(path.clone()), &[]);
        first.record("git status").unwrap();
        first.record("git status").unwrap();
        second.record("git commit -m x").unwrap();
        second.record("git commit -m x").unwrap();
        second.record("git commit -m x").unwrap();
        second.sync().unwrap();
        first.sync().unwrap();
        assert_eq!(first.uses("git status"), Some(2));
        assert_eq!(first.uses("git commit -m x"), Some(3));
        assert_eq!(second.uses("git status"), Some(2));
        assert_eq!(first.suggest("git c"), Some("ommit -m x"));
        assert_eq!(second.suggest("git s"), Some("tatus"));
        // A late window reads the whole file.
        let late = History::open(Some(path), &[]);
        assert_eq!(late.uses("git commit -m x"), Some(3));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn credential_bearing_commands_are_never_admissible() {
        for secret in [
            "export GITHUB_TOKEN=abc123",
            "API_KEY=xyz npm run deploy",
            "mysql -psecret123 -u root",
            "mysql --password=hunter2",
            "mysql --password hunter2",
            "curl -u alice:hunter2 https://x.example",
            "git clone https://alice:hunter2@git.example/repo.git",
            "sudo -S apt update",
            "sudo -kS whoami",
            "echo hi | passwd alice",
            "aws configure set aws_secret_access_key abc",
            "Authorization: Bearer abc",
        ] {
            assert!(!admissible(secret), "must be filtered: {secret}");
        }
        for plain in ["ls -la", "git status", "sudo apt update", "cargo test -p flamewm-terminal", "ssh host", "cd /tmp"] {
            assert!(admissible(plain), "must be kept: {plain}");
        }
    }

    #[test]
    fn stored_secrets_are_removed_when_the_store_loads() {
        let dir = tmp_dir("purge");
        let path = dir.join("terminal-history");
        fs::write(&path, "2\tls\n1\texport TOKEN=abc\n1\tcurl https://a:b@host/x\n3\tgit status\n").unwrap();
        let history = History::open(Some(path.clone()), &[]);
        assert_eq!(history.uses("ls"), Some(2));
        assert_eq!(history.uses("export TOKEN=abc"), None);
        assert_eq!(fs::read_to_string(&path).unwrap(), "2\tls\n3\tgit status\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn filtered_commands_are_not_persisted() {
        let dir = tmp_dir("filter");
        let path = dir.join("terminal-history");
        let mut history = History::open(Some(path.clone()), &[]);
        history.record(" export SECRET=1").unwrap();
        history.record("curl -u a password=b").unwrap();
        history.record("ls").unwrap();
        assert_eq!(history.uses("ls"), Some(1));
        assert_eq!(history.uses(" export SECRET=1"), None);
        let body = fs::read_to_string(&path).unwrap();
        assert_eq!(body, "1\tls\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn compaction_keeps_counts_and_other_windows_reload() {
        let dir = tmp_dir("compact");
        let path = dir.join("terminal-history");
        let mut history = History::open(Some(path.clone()), &[]);
        let mut other = History::open(Some(path.clone()), &[]);
        history.record("make").unwrap();
        // Force a compaction by writing past the limit with lines that aggregate to one command.
        let filler = "1\tmake\n".repeat(200_000);
        let mut body = fs::read_to_string(&path).unwrap();
        body.push_str(&filler);
        fs::write(&path, body).unwrap();
        other.sync().unwrap();
        history.record("make").unwrap(); // size is over the limit: compacts
        other.sync().unwrap();
        assert_eq!(history.uses("make"), Some(200_002));
        assert_eq!(other.uses("make"), Some(200_002));
        assert!(fs::metadata(&path).unwrap().len() < 100, "compacted to one line");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_store_of_distinct_commands_is_not_recompacted_on_every_record() {
        let dir = tmp_dir("distinct");
        let path = dir.join("terminal-history");
        let mut history = History::open(Some(path.clone()), &[]);
        // Distinct commands cannot fold, so compaction leaves the store above COMPACT_BYTES.
        let body: String = (0..40_000).map(|i| format!("1\tcommand-{i:08}-padding-padding-padding\n")).collect();
        fs::write(&path, body).unwrap();
        history.record("ls").unwrap();
        let after = fs::metadata(&path).unwrap().len();
        assert!(after > COMPACT_BYTES, "still above the limit after folding: {after}");
        assert!(history.compact_at >= after * 2, "next compaction waits for the store to double: {}", history.compact_at);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn first_run_seeds_from_shell_history_read_only() {
        let dir = tmp_dir("seed");
        let zsh = dir.join("zsh_history");
        let bash = dir.join("bash_history");
        fs::write(&zsh, ": 1700000000:0;git status\n: 1700000001:0;git status\n: 1700000002:0;echo hi\n: 1700000003:0; secret password=1\n").unwrap();
        fs::write(&bash, "#1700000010\nls -la\nls -la\n  \\\n  multi\n").unwrap();
        let path = dir.join("store");
        let history = History::open(Some(path.clone()), &[zsh.clone(), bash.clone()]);
        assert_eq!(history.uses("git status"), Some(2));
        assert_eq!(history.uses("echo hi"), Some(1));
        assert_eq!(history.uses("ls -la"), Some(2));
        assert_eq!(history.uses("secret password=1"), None);
        assert_eq!(history.suggest("git s"), Some("tatus"));
        assert_eq!(fs::read_to_string(&zsh).unwrap().lines().count(), 4, "history files untouched");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recreated_store_reloads_from_scratch() {
        let dir = tmp_dir("inode");
        let path = dir.join("terminal-history");
        let mut history = History::open(Some(path.clone()), &[]);
        history.record("old").unwrap();
        assert_eq!(history.uses("old"), Some(1));
        fs::remove_file(&path).unwrap();
        fs::write(&path, "4\tnew\n").unwrap();
        history.sync().unwrap();
        assert_eq!(history.uses("old"), None);
        assert_eq!(history.uses("new"), Some(4));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_last_line_waits_for_its_newline() {
        let dir = tmp_dir("partial");
        let path = dir.join("terminal-history");
        fs::write(&path, "1\tdone\n2\tpart").unwrap();
        let mut history = History::open(Some(path.clone()), &[]);
        assert_eq!(history.uses("part"), None);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"ial\n").unwrap();
        history.sync().unwrap();
        assert_eq!(history.uses("partial"), Some(2));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn line_tracks_typing_editing_and_enter() {
        let mut line = LineInput::default();
        type_text(&mut line, "git committ");
        assert!(line.is_ready());
        assert_eq!(line.key("backspace", Mods::default(), None), None);
        assert_eq!(line.text(), "git commit");
        assert_eq!(line.key("return", Mods::default(), Some("\r")), Some("git commit".to_owned()));
        assert_eq!(line.text(), "", "Enter starts an empty line");
        assert!(!line.is_ready(), "an empty line has no suggestion");
    }

    #[test]
    fn line_handles_cursor_movement_and_kills() {
        let mut line = LineInput::default();
        type_text(&mut line, "abc");
        line.key("left", Mods::default(), None);
        assert!(!line.is_ready(), "cursor left of the end hides the suggestion");
        line.key("end", Mods::default(), None);
        assert!(line.is_ready());
        line.key("a", ctrl(), None);
        type_text(&mut line, "x");
        assert_eq!(line.text(), "xabc");
        line.key("e", ctrl(), None);
        line.key("k", ctrl(), None);
        assert_eq!(line.text(), "xabc");
        line.key("u", ctrl(), None);
        assert_eq!(line.text(), "");
        type_text(&mut line, "ls");
        line.key("c", ctrl(), None);
        assert_eq!(line.text(), "", "Ctrl+C discards the line");
        assert!(!line.is_ready(), "an empty line is not ready");
        type_text(&mut line, "l");
        assert!(line.is_ready(), "typing again after Ctrl+C is tracked");
    }

    #[test]
    fn unknown_lines_are_not_recorded_or_suggested() {
        let mut line = LineInput::default();
        type_text(&mut line, "gi");
        line.key("up", Mods::default(), None);
        type_text(&mut line, "t");
        assert!(!line.is_ready(), "history recall makes the line unknown");
        assert_eq!(line.key("return", Mods::default(), Some("\r")), None, "unknown line is not submitted");
        type_text(&mut line, "ls");
        assert_eq!(line.key("return", Mods::default(), Some("\r")), Some("ls".to_owned()), "a new line is known again");
    }

    #[test]
    fn tab_completion_and_ctrl_w_make_the_line_unknown() {
        let mut line = LineInput::default();
        type_text(&mut line, "cd sr");
        line.key("tab", Mods::default(), Some("\t"));
        assert!(!line.is_ready());
        let mut line = LineInput::default();
        type_text(&mut line, "cd src");
        line.key("w", ctrl(), None);
        assert!(!line.is_ready());
    }

    #[test]
    fn paste_inserts_plain_text_and_marks_multiline_unknown() {
        let mut line = LineInput::default();
        line.paste("echo one");
        assert_eq!(line.text(), "echo one");
        assert!(line.is_ready());
        line.paste("two\nthree");
        assert!(!line.is_ready());
    }

    #[test]
    fn accepted_suffix_is_appended_to_the_line() {
        let mut line = LineInput::default();
        type_text(&mut line, "git ");
        line.insert_str("commit");
        assert_eq!(line.text(), "git commit");
        assert!(line.is_ready());
    }

    #[test]
    fn history_suggestion_is_a_suffix_of_the_best_command() {
        let dir = tmp_dir("suffix");
        let path = dir.join("terminal-history");
        fs::write(&path, "1\tgit commit -m \"test\"\n5\tgit status\n").unwrap();
        let history = History::open(Some(path), &[]);
        assert_eq!(history.suggest("g"), Some("it status"));
        assert_eq!(history.suggest("git commit -m \"test\""), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
