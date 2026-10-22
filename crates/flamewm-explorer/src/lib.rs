//! Filesystem and navigation model for the native FlameWM explorer.
//! Filesystem work must be performed by the worker; the UI thread only projects snapshots.
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::os::unix::fs::symlink;
use std::time::SystemTime;

pub mod archive;
pub mod places;
pub mod prefs;
pub mod properties;
pub mod refresh;
pub mod search;
pub mod trash;
pub mod view;

pub mod jobs;
/// Row widgets in the template. The list viewport decides how many of them are shown.
pub const ROW_SLOTS: usize = 24;
pub const MAX_LISTED_ENTRIES: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy { Name, Size, Modified }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}
impl Entry {
    pub fn size_label(&self) -> String {
        if self.directory { return "—".into(); }
        view::human_size(self.size)
    }
    pub fn modified_label(&self) -> String {
        self.modified.map_or_else(|| "—".into(), |m| view::date_label(m, chrono::Local::now()))
    }
}

pub fn list_directory(dir: &Path, show_hidden: bool, sort: SortBy) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') { continue; }
        let path = item.path();
        let meta = fs::symlink_metadata(&path)?;
        let symlink = meta.file_type().is_symlink();
        let directory = meta.is_dir() || (symlink && path.is_dir());
        entries.push(Entry { path, name, directory, symlink,
            size: meta.len(), modified: meta.modified().ok() });
        if entries.len() >= MAX_LISTED_ENTRIES { break; }
    }
    entries.sort_by(|a,b| {
        b.directory.cmp(&a.directory).then_with(|| match sort {
            SortBy::Name => a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()),
            SortBy::Size => a.size.cmp(&b.size),
            SortBy::Modified => b.modified.cmp(&a.modified),
        }).then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub selected_paths: HashSet<PathBuf>,
    history: Vec<PathBuf>,
    cursor: usize,
    pub entries: Vec<Entry>,
    pub selected: Option<PathBuf>,
    anchor: Option<PathBuf>,
    /// Index of the first filtered row in the window.
    pub scroll: usize,
    pub search: String,
    pub loading: bool,
}
impl Tab {
    pub fn new(path: PathBuf) -> Self { Self { selected_paths:HashSet::new(), history: vec![path], cursor:0, entries:Vec::new(), selected:None, anchor:None, scroll:0, search:String::new(), loading:true } }
    pub fn path(&self) -> &Path { &self.history[self.cursor] }
    pub fn visit(&mut self, path: PathBuf) {
        self.history.truncate(self.cursor + 1);
        self.history.push(path); self.cursor += 1; self.reset();
    }
    pub fn back(&mut self) -> bool { if self.cursor == 0 { return false; } self.cursor -= 1; self.reset(); true }
    pub fn forward(&mut self) -> bool { if self.cursor + 1 >= self.history.len() { return false; } self.cursor += 1; self.reset(); true }
    pub fn up(&mut self) -> bool { if let Some(p)=self.path().parent().map(Path::to_path_buf) { if p != self.path() { self.visit(p); return true; } } false }
    pub fn can_back(&self) -> bool { self.cursor > 0 }
    pub fn can_forward(&self) -> bool { self.cursor + 1 < self.history.len() }
    fn reset(&mut self) { self.entries.clear(); self.selected_paths.clear(); self.selected=None; self.anchor=None; self.scroll=0; self.search.clear(); self.loading=true; }
    /// Filter once per request, retaining at most the currently rendered page.
    /// Avoid allocating a Vec of every match on each UI projection.
    pub fn select_one(&mut self, path: PathBuf) {
        self.selected_paths.clear();
        self.selected_paths.insert(path.clone());
        self.selected=Some(path.clone());
        self.anchor=Some(path);
    }
    /// Toggle an entry without discarding the current selection.
    pub fn toggle_selection(&mut self, path: PathBuf) {
        if !self.selected_paths.insert(path.clone()) {
            self.selected_paths.remove(&path);
        }
        self.selected=self.selected_paths.contains(&path).then_some(path.clone())
            .or_else(||self.selected_paths.iter().min().cloned());
        self.anchor=self.selected.clone();
    }
    /// Extend a contiguous selection in the filtered/sorted display order.
    pub fn select_range_to(&mut self, target: PathBuf) {
        self.select_range_to_with_mode(target, false);
    }
    /// Shift replaces the selection; Ctrl+Shift unions the range with it.
    /// The anchor stays at the original click, including after repeated range updates.
    pub fn select_range_to_with_mode(&mut self, target: PathBuf, additive: bool) {
        let Some(target_index)=self.visible_index_of(&target) else {return};
        let Some(start)=self.anchor.as_deref().and_then(|p|self.visible_index_of(p)) else {
            if additive {self.toggle_selection(target);} else {self.select_one(target);}
            return;
        };
        let query=self.search.to_lowercase();
        let range=self.entries.iter().filter(|e| {
            query.is_empty() || e.name.to_lowercase().contains(&query)
        }).skip(start.min(target_index)).take(start.abs_diff(target_index)+1)
            .map(|e|e.path.clone());
        if !additive {self.selected_paths.clear();}
        self.selected_paths.extend(range);
        self.selected=Some(target);
    }
    pub fn select_all_filtered(&mut self) {
        let query=self.search.to_lowercase();
        self.selected_paths=self.entries.iter()
            .filter(|e|query.is_empty() || e.name.to_lowercase().contains(&query))
            .map(|e|e.path.clone()).collect();
        self.selected=self.entries.iter().find(|e|self.selected_paths.contains(&e.path)).map(|e|e.path.clone());
        self.anchor=self.selected.clone();
    }
    pub fn ordered_selection(&self) -> Vec<PathBuf> {
        // Never depend on HashSet iteration order for transfer job ordering.
        let mut paths=self.selected_paths.iter().cloned().collect::<Vec<_>>();
        paths.sort();paths
    }
    pub fn clear_selection(&mut self) { self.selected_paths.clear();self.selected=None;self.anchor=None; }
    /// Select the window rows `from_row..=to_row` (slots relative to the scroll offset).
    pub fn select_vertical_band(&mut self, from_row: usize, to_row: usize, rows: usize) {
        let (lo,hi)=(from_row.min(to_row),from_row.max(to_row));
        let paths=self.visible_window(rows).into_iter().enumerate()
            .filter(|(i,_)| (lo..=hi).contains(i))
            .map(|(_,entry)|entry.path.clone()).collect::<Vec<_>>();
        self.selected_paths=paths.iter().cloned().collect();
        self.selected=paths.first().cloned();
        self.anchor=self.selected.clone();
    }
    /// Filtered rows starting at the scroll offset, at most `rows` of them.
    pub fn visible_window(&self, rows: usize) -> Vec<&Entry> {
        let query=self.search.to_lowercase();
        self.entries.iter()
            .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .skip(self.window_start(rows))
            .take(rows)
            .collect()
    }
    /// Scroll offset clamped to the filtered list, so a shorter list never shows blank space.
    pub fn window_start(&self, rows: usize) -> usize {
        self.scroll.min(self.filtered_count().saturating_sub(rows))
    }
    /// Move the window by whole rows; the offset stays within the list.
    pub fn scroll_by(&mut self, delta: isize, rows: usize) {
        let max=self.filtered_count().saturating_sub(rows) as isize;
        self.scroll=(self.window_start(rows) as isize + delta).clamp(0,max) as usize;
    }
    /// Scroll the minimum amount that keeps filtered row `index` inside the window. `rows >= 1`.
    pub fn reveal(&mut self, index: usize, rows: usize) {
        let start=self.window_start(rows);
        self.scroll=if index<start {index} else if index>=start+rows {index+1-rows} else {start};
    }
    pub fn visible_nth(&self, index:usize) -> Option<&Entry> {
        let query=self.search.to_lowercase();
        self.entries.iter()
            .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .nth(index)
    }
    pub fn visible_index_of(&self, path:&Path) -> Option<usize> {
        let query=self.search.to_lowercase();
        self.entries.iter()
            .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .position(|entry| entry.path==path)
    }
    pub fn filtered_count(&self) -> usize {
        let query=self.search.to_lowercase();
        self.entries.iter()
            .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .count()
    }
}
#[derive(Debug)]
pub struct Explorer {
    pub tabs: Vec<Tab>, pub active: usize, pub generation: u64,
    pub hidden: bool, pub sort: SortBy, pub status: String, pub clipboard: Option<(Vec<PathBuf>, bool)>,
}
impl Explorer {
    pub fn new(path: PathBuf) -> Self { Self { tabs: vec![Tab::new(path)], active:0, generation:1, hidden:false, sort:SortBy::Name, status:"Loading…".into(), clipboard:None } }
    pub fn tab(&self) -> &Tab { &self.tabs[self.active] }
    pub fn tab_mut(&mut self) -> &mut Tab { &mut self.tabs[self.active] }
    pub fn switch_tab(&mut self, index:usize) -> bool { if index>=self.tabs.len() { false } else {self.active=index;true} }
    pub fn close_tab(&mut self) { if self.tabs.len()>1 {self.tabs.remove(self.active);self.active=self.active.min(self.tabs.len()-1);} }
    pub fn new_tab(&mut self) { let path=self.tab().path().to_path_buf(); self.tabs.push(Tab::new(path)); self.active=self.tabs.len()-1; self.generation+=1; }
    pub fn next_generation(&mut self) -> u64 { self.generation+=1; self.tab_mut().loading=true; self.generation }
    pub fn apply_listing(&mut self, generation:u64, dir:&Path, listing:io::Result<Vec<Entry>>) -> bool {
        if generation!=self.generation || self.tab().path()!=dir { return false; }
        match listing {
            Ok(items) => {
                // The status strip shows the count itself (view::items_text), so the message stays free for results.
                self.status.clear();
                let tab=self.tab_mut();
                if !tab.selected_paths.is_empty() {
                    let present=items.iter().map(|e|e.path.as_path()).collect::<HashSet<_>>();
                    tab.selected_paths.retain(|path|present.contains(path.as_path()));
                }
                if !tab.selected.as_ref().is_some_and(|p|tab.selected_paths.contains(p)) {
                    tab.selected=tab.selected_paths.iter().min().cloned();
                }
                if !tab.anchor.as_ref().is_some_and(|p|tab.selected_paths.contains(p)) {tab.anchor=tab.selected.clone();}
                tab.entries=items;
            }
            Err(e) => self.status=format!("Cannot read directory: {e}"),
        }
        self.tab_mut().loading=false;
        true
    }
}
/// Sidebar section a place belongs to. Sections render in declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlaceGroup { Favorites, Devices, Pinned }

impl PlaceGroup {
    pub fn heading(self) -> &'static str {
        match self { Self::Favorites => "Favorites", Self::Devices => "Devices", Self::Pinned => "Pinned Folders" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub name:String,
    /// Empty for an unmounted device: activating it mounts the device instead of navigating.
    pub path:PathBuf,
    pub removable:bool,
    pub group:PlaceGroup,
    /// Line-icon family for the sidebar (`ui/explorer/icons/<icon>.svg`).
    pub icon:&'static str,
    /// Block device (`/dev/sdb1`) for removable media, mounted or not; drives udisks2 actions.
    pub device:Option<String>,
}
impl Place {
    pub fn mounted(&self)->bool {!self.path.as_os_str().is_empty()}
}

/// One sidebar row: a group heading, or an index into the places list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarRow { Heading(&'static str), Place(usize) }

/// Groups places into sidebar rows. A heading is emitted only for a non-empty group.
pub fn sidebar_rows(places:&[Place]) -> Vec<SidebarRow> {
    let mut rows=Vec::with_capacity(places.len()+3);
    for group in [PlaceGroup::Favorites,PlaceGroup::Devices,PlaceGroup::Pinned] {
        let members:Vec<usize>=places.iter().enumerate().filter(|(_,p)|p.group==group).map(|(i,_)|i).collect();
        if members.is_empty() {continue;}
        rows.push(SidebarRow::Heading(group.heading()));
        rows.extend(members.into_iter().map(SidebarRow::Place));
    }
    rows
}
pub fn home_dir() -> PathBuf {std::env::var_os("HOME").map(PathBuf::from).filter(|p|p.is_dir()).unwrap_or_else(||PathBuf::from("/"))}
pub fn user_dir(name:&str, fallback:&str, home:&Path) -> PathBuf {
    let base=std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".config"));
    let content=fs::read_to_string(base.join("user-dirs.dirs")).unwrap_or_default();
    for line in content.lines() {
        if let Some((key,val))=line.trim().split_once('=') {
            if key==format!("XDG_{name}_DIR") {
                let v=val.trim().trim_matches('"');
                if v=="$HOME" {return home.to_path_buf();}
                if let Some(r)=v.strip_prefix("$HOME/") { if !r.split('/').any(|p|p=="..") {return home.join(r);} }
            }
        }
    }
    home.join(fallback)
}
pub fn add_bookmark(path:&Path,home:&Path)->io::Result<()> {
    let base=std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".config"));
    let dest=base.join("gtk-3.0/bookmarks");
    let escaped=path.to_string_lossy().bytes().map(|b|if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {(b as char).to_string()}else{format!("%{b:02X}")}).collect::<String>();
    let uri=format!("file://{escaped}");
    let current=fs::read_to_string(&dest).unwrap_or_default();
    if current.lines().any(|line|line.split_whitespace().next()==Some(uri.as_str())) {return Ok(());}
    fs::create_dir_all(dest.parent().unwrap_or(home))?;
    use std::fs::OpenOptions;
    let mut file=OpenOptions::new().append(true).create(true).open(dest)?;
    writeln!(file,"{uri}")
}
fn unescape_uri(raw:&str)->Option<PathBuf>{
    let raw=raw.strip_prefix("file://")?;
    let mut result=Vec::with_capacity(raw.len());
    let mut bytes=raw.bytes();
    while let Some(b)=bytes.next() {
        if b==b'%' {
            let c=bytes.next()?;let d=bytes.next()?;
            let digit=|v:u8| -> Option<u8> {match v {b'0'..=b'9'=>Some(v-b'0'),b'a'..=b'f'=>Some(v-b'a'+10),b'A'..=b'F'=>Some(v-b'A'+10),_=>None}};
            result.push(digit(c)?*16+digit(d)?);
        } else {result.push(b);}
    }
    String::from_utf8(result).ok().map(PathBuf::from)
}
fn favorite(name:&str,path:PathBuf,icon:&'static str)->Place {
    Place{name:name.into(),path,removable:false,group:PlaceGroup::Favorites,icon,device:None}
}
/// Sidebar places: built-in folders, bookmarks, mounted removable media (from mountinfo) and
/// removable block devices that are present but not mounted (from `lsblk`, passed in by the caller).
pub fn places(home:&Path,unmounted:&[places::BlockDevice]) -> Vec<Place> {
    let mut places=vec![favorite("Home",home.to_path_buf(),"home")];
    for (id,name,dir,icon) in [("DESKTOP","Desktop","Desktop","desktop"),("DOCUMENTS","Documents","Documents","documents"),("DOWNLOAD","Downloads","Downloads","downloads"),("MUSIC","Music","Music","music"),("PICTURES","Pictures","Pictures","pictures"),("VIDEOS","Videos","Videos","videos")] {
        places.push(favorite(name,user_dir(id,dir,home),icon));
    }
    places.push(Place{name:"Local Disk".into(),path:PathBuf::from("/"),removable:false,group:PlaceGroup::Devices,icon:"drive",device:None});
    let base=std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".config"));
    if let Ok(source)=fs::read_to_string(base.join("gtk-3.0/bookmarks")) {
        for line in source.lines().take(128) {
            let mut words=line.splitn(2,' ');
            if let Some(path)=words.next().and_then(unescape_uri).filter(|p|p.is_dir()) {
                let name=words.next().map(str::to_owned).unwrap_or_else(||path.file_name().unwrap_or_default().to_string_lossy().into_owned());
                places.push(Place{name,path,removable:false,group:PlaceGroup::Pinned,icon:"folder",device:None});
            }
        }
    }
    if let Ok(contents)=fs::read_to_string("/proc/self/mountinfo") {
        for device in places::parse_mountinfo(&contents) {
            if device.mount_point.is_dir() {
                let name=places::sanitize_label(&device.mount_point.file_name().unwrap_or_default().to_string_lossy());
                // Nameless mounts are not listed, matching Dolphin's sidebar.
                if name.is_empty() {continue;}
                places.push(Place{name,path:device.mount_point,removable:true,group:PlaceGroup::Devices,icon:"usb",device:Some(format!("/dev/{}",device.block_name))});
            }
        }
    }
    places.extend(unmounted.iter().map(unmounted_place));
    places
}

/// Sidebar row for a present but unmounted removable filesystem: no path, so `mounted()` is false.
pub fn unmounted_place(device:&places::BlockDevice)->Place {
    Place{name:device.label.clone(),path:PathBuf::new(),removable:true,group:PlaceGroup::Devices,icon:"usb",device:Some(device.device.clone())}
}

/// Crash-safe copy owned by `jobs`; never overwrites an existing entry.
pub fn copy_entry(from: &Path, to: &Path, report: &mut impl FnMut(u64)) -> io::Result<()> {
    jobs::copy_publish(from, to, false, &jobs::CancellationToken::new(), report)
}
pub fn destination_for(from: &Path, directory: &Path) -> PathBuf {
    let name = from.file_name().unwrap_or_default().to_string_lossy();
    let target = directory.join(name.as_ref());
    if fs::symlink_metadata(&target).is_err() { return target; }
    for index in 2..=10_000 {
        let target = directory.join(format!("{name} ({index})"));
        if fs::symlink_metadata(&target).is_err() { return target; }
    }
    directory.join(format!("{name} (overflow)"))
}
/// Summary of a batch operation; errors remain visible even when other items succeed.
#[derive(Debug,Default,PartialEq,Eq)]
pub struct BatchReport {
    pub completed: usize,
    pub failed: usize,
    pub first_error: Option<String>,
    pub failed_sources: Vec<PathBuf>,
    pub skipped: usize,
    pub cancelled: bool,
    pub undo: Vec<jobs::Undo>,
}
impl BatchReport {
    pub fn record(&mut self, source:&Path, result:io::Result<()>) {
        match result {
            Ok(())=>self.completed+=1,
            Err(error)=>{
                self.failed+=1;
                self.failed_sources.push(source.to_path_buf());
                if self.first_error.is_none() {self.first_error=Some(format!("{}: {error}",source.display()));}
            }
        }
    }
    pub fn append(&mut self, other:BatchReport) {
        self.completed+=other.completed;
        self.failed+=other.failed;
        if self.first_error.is_none() {self.first_error=other.first_error;}
        self.failed_sources.extend(other.failed_sources);
        self.skipped+=other.skipped;
        self.cancelled|=other.cancelled;
        self.undo.extend(other.undo);
    }
    pub fn message(&self, verb:&str)->String {
        let mut text=self.summary(verb);
        if self.skipped>0 {text.push_str(&format!(", {} skipped",self.skipped));}
        if self.cancelled {text=format!("Cancelled: {text}");}
        text
    }
    fn summary(&self, verb:&str)->String {
        if self.failed==0 { format!("{verb}: {} item(s)",self.completed) }
        else {format!("{verb}: {} succeeded, {} failed — {}",self.completed,self.failed,self.first_error.as_deref().unwrap_or("unknown error"))}
    }
}

/// Transfer every selected top-level entry; never overwrite, and continue past errors.
/// Renames across devices use the existing safe-copy fallback.
/// Validate once on the worker: even an alias to a descendant must not become
/// a recursive copy or a move into itself. The UI must not do filesystem I/O.
pub fn validate_transfer_destination(source:&Path, directory:&Path) -> io::Result<()> {
    let metadata=fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        let real_source=fs::canonicalize(source)?;
        let real_directory=fs::canonicalize(directory)?;
        if real_directory.starts_with(real_source) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "cannot transfer a directory into itself or its descendant"));
        }
    }
    Ok(())
}

pub fn transfer_batch(sources:&[PathBuf], directory:&Path, move_it:bool, report:&mut impl FnMut(u64)) -> BatchReport {
    jobs::transfer_batch_with(sources, directory, move_it, jobs::Conflict::KeepBoth, &jobs::CancellationToken::new(), report)
}

/// A drop follows the file-manager convention: move within one filesystem,
/// copy across filesystem boundaries; Ctrl forces a copy. Cut/Paste remains
/// an explicit move and is not affected by this policy.
pub fn drop_moves(force_copy:bool, source_dev:u64, destination_dev:u64)->bool {
    !force_copy && source_dev==destination_dev
}

pub fn transfer_drop_batch(sources:&[PathBuf], directory:&Path, force_copy:bool,
    report:&mut impl FnMut(u64)) -> BatchReport {
    jobs::transfer_drop_batch_with(sources, directory, force_copy, &jobs::CancellationToken::new(), report)
}

/// Move owned by `jobs`: no-replace rename, verified cross-device copy fallback.
pub fn move_entry(from: &Path, to: &Path, report: &mut impl FnMut(u64)) -> io::Result<()> {
    jobs::move_publish(from, to, false, &jobs::CancellationToken::new(), report)
}
pub fn display_location(path:&Path, home:&Path)->String {
    if path==home {return "~".into();}
    if let Ok(rest)=path.strip_prefix(home) {return format!("~/{}",rest.display());}
    path.display().to_string()
}
pub enum AddressTarget { Local(PathBuf), ExternalUri(String) }
pub fn parse_address(text:&str, home:&Path)->Result<AddressTarget,String> {
    let trimmed=text.trim();
    if trimmed=="~" {return Ok(AddressTarget::Local(home.to_path_buf()));}
    if let Some(rest)=trimmed.strip_prefix("~/") {return Ok(AddressTarget::Local(home.join(rest)));}
    if trimmed.starts_with("file://") {
        return unescape_uri(trimmed).filter(|p|p.is_absolute())
            .map(AddressTarget::Local).ok_or_else(||"Invalid file URI".to_owned());
    }
    if let Some((scheme,tail))=trimmed.split_once("://") {
        if ["sftp","ssh","smb","ftp","ftps","dav","davs","mtp","nfs"].contains(&scheme.to_ascii_lowercase().as_str())
            && !tail.is_empty() && !tail.chars().any(char::is_whitespace) {
            return Ok(AddressTarget::ExternalUri(trimmed.to_owned()));
        }
        return Err("Unsupported or incomplete remote URL".into());
    }
    if trimmed.starts_with('/') {return Ok(AddressTarget::Local(PathBuf::from(trimmed)));}
    Err("Enter an absolute path, ~/path, or supported remote URL".into())
}
pub fn is_archive(path:&Path)->bool {
    let name=path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    [".zip",".7z",".rar",".tar",".tar.gz",".tgz",".tar.bz2",".tbz2",".tar.xz",".txz",".tar.zst",".jar",".deb"]
        .iter().any(|suffix|name.ends_with(suffix))
}
#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;
    use super::*;
    #[test] fn lsblk_unmounted_partition_maps_to_unmounted_device_place() {
        let sample = r#"{"blockdevices":[{"name":"sdb","rm":true,"hotplug":true,"children":[
            {"name":"sdb1","rm":true,"hotplug":true,"fstype":"vfat","mountpoint":null,"label":"Backup"}]}]}"#;
        let devices = places::parse_lsblk(sample);
        let rows: Vec<Place> = devices.iter().map(unmounted_place).collect();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].mounted());
        assert_eq!(rows[0].name, "Backup");
        assert_eq!(rows[0].device.as_deref(), Some("/dev/sdb1"));
        assert_eq!(rows[0].group, PlaceGroup::Devices);
    }
    #[test] fn flamewm_explorer_desktop_entry_is_listed_in_system_menu_bucket() {
        let dir = std::env::temp_dir().join(format!("flamewm-explorer-xdg-{}", std::process::id()));
        let apps = dir.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        let exe = std::env::current_exe().unwrap();
        let text = include_str!("../../../packaging/flamewm-explorer.desktop")
            .replace("Exec=flamewm-explorer", &format!("Exec={}", exe.display()))
            .replace("TryExec=flamewm-explorer", &format!("TryExec={}", exe.display()));
        std::fs::write(apps.join("flamewm-explorer.desktop"), text).unwrap();
        let catalog = flamewm_applications::ApplicationCatalog::from_dirs(&[apps]).unwrap();
        let app = catalog.all().into_iter().find(|a| a.name == "FlameWM Explorer");
        std::fs::remove_dir_all(&dir).unwrap();
        let app = app.expect("explorer entry must survive NoDisplay/TryExec filtering");
        assert_eq!(app.categories.first().map(String::as_str), Some("System"));
    }
    #[test] fn home_alias_and_remote_protocols_do_not_confuse_native_paths() {
        let home=Path::new("/home/demo");
        assert_eq!(display_location(home,home),"~");
        assert_eq!(display_location(&home.join("Documents"),home),"~/Documents");
        assert_eq!(display_location(Path::new("/etc"),home),"/etc");
        assert!(matches!(parse_address("~",home),Ok(AddressTarget::Local(p)) if p==home));
        assert!(matches!(parse_address("sftp://host/home/demo",home),Ok(AddressTarget::ExternalUri(_))));
        assert!(matches!(parse_address("smb://server/share",home),Ok(AddressTarget::ExternalUri(_))));
        assert!(parse_address("javascript://evil",home).is_err());
        assert!(parse_address("sftp://",home).is_err());
    }
    #[test] fn archive_names_and_marquee_selection_respect_filtered_rows() {
        assert!(is_archive(Path::new("work.TAR.GZ")));
        assert!(is_archive(Path::new("photo.zip")));
        assert!(!is_archive(Path::new("photo.png")));
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.entries=(0..6).map(|i|Entry{path:PathBuf::from(format!("/tmp/file{i}")),name:format!("file{i}"),directory:false,symlink:false,size:0,modified:None}).collect();
        tab.select_vertical_band(1,3,ROW_SLOTS);
        assert_eq!(tab.selected_paths.len(),3);
        assert!(tab.selected_paths.contains(Path::new("/tmp/file2")));
        tab.select_one(PathBuf::from("/tmp/file4"));
        assert_eq!(tab.selected_paths.len(),1);
        tab.visit(PathBuf::from("/tmp/other"));
        assert!(tab.selected_paths.is_empty());
    }
    #[test] fn history_and_tabs_do_not_lose_locations() {
        let mut model=Explorer::new(PathBuf::from("/tmp"));model.tab_mut().visit(PathBuf::from("/"));
        assert!(model.tab_mut().back());assert_eq!(model.tab().path(),Path::new("/tmp"));
        assert!(model.tab_mut().forward());model.new_tab();model.tab_mut().visit(PathBuf::from("/tmp"));model.switch_tab(0);assert_eq!(model.tab().path(),Path::new("/"));
    }
    #[test]
    fn mouse_selection_ctrl_toggle_shift_anchor_and_ctrl_shift_union() {
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.entries=(0..6).map(|i| Entry {
            path:PathBuf::from(format!("/tmp/item{i}")),
            name:format!("item{i}"),directory:false,symlink:false,size:0,modified:None,
        }).collect();
        let item=|index| PathBuf::from(format!("/tmp/item{index}"));
        tab.select_one(item(1));
        tab.toggle_selection(item(5));
        assert_eq!(tab.selected_paths.len(),2);
        tab.toggle_selection(item(5));
        assert_eq!(tab.selected_paths.len(),1);
        tab.select_one(item(1));
        tab.select_range_to(item(3));
        assert_eq!(tab.selected_paths.len(),3);
        assert!(tab.selected_paths.contains(&item(1)));
        assert!(tab.selected_paths.contains(&item(3)));
        tab.select_one(item(0));
        tab.toggle_selection(item(5));
        tab.select_range_to_with_mode(item(3),true);
        assert_eq!(tab.selected_paths.len(),4); // original 0 plus range 3..5
        assert_eq!(tab.ordered_selection(),vec![item(0),item(3),item(4),item(5)]);
    }

    #[test] fn scroll_window_follows_viewport_and_keeps_indices() {
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.loading=false;
        tab.entries=(0..100).map(|n| Entry {
            path:PathBuf::from(format!("/tmp/item-{n:03}")), name:format!("item-{n:03}"),
            directory:false,symlink:false,size:n,modified:None,
        }).collect();
        let rows=12;
        tab.scroll_by(20,rows);
        assert_eq!(tab.scroll,20);
        let window=tab.visible_window(rows);
        assert_eq!(window.len(),rows);
        assert_eq!(window[0].name,"item-020");
        assert_eq!(tab.visible_index_of(&window[0].path),Some(20));
        // Keyboard reveal scrolls only as far as needed: index 31 is one past the window.
        tab.reveal(31,rows);
        assert_eq!(tab.scroll,20);
        tab.reveal(32,rows);
        assert_eq!(tab.scroll,21);
        tab.reveal(0,rows);
        assert_eq!(tab.scroll,0);
        // Scrolling clamps at the last full window.
        tab.scroll_by(1000,rows);
        assert_eq!(tab.scroll,88);
        assert_eq!(tab.visible_window(rows)[rows-1].name,"item-099");
        tab.scroll_by(-1000,rows);
        assert_eq!(tab.scroll,0);
        // A filter that shrinks the list pulls a stale offset back into range.
        tab.scroll=50;
        tab.search="item-09".into();
        assert_eq!(tab.filtered_count(),10);
        assert_eq!(tab.visible_window(rows).len(),10);
        assert_eq!(tab.visible_window(rows)[0].name,"item-090");
    }
    #[test] fn stale_worker_reply_is_discarded() {
        let mut model=Explorer::new(PathBuf::from("/tmp"));let old=model.generation;model.next_generation();
        assert!(!model.apply_listing(old,Path::new("/tmp"),Ok(Vec::new())));
    }
    #[test] fn move_refuses_existing_destination_and_preserves_both_files() {
        let base = std::env::temp_dir().join(format!("flamewm-explorer-noreplace-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("source"), b"source").unwrap();
        fs::write(base.join("existing"), b"existing").unwrap();
        assert!(move_entry(&base.join("source"), &base.join("existing"), &mut |_| {}).is_err());
        assert_eq!(fs::read(base.join("source")).unwrap(), b"source");
        assert_eq!(fs::read(base.join("existing")).unwrap(), b"existing");
        fs::remove_dir_all(base).unwrap();
    }
    #[test] fn keyboard_range_toggle_and_select_all_follow_filtered_order() {
        let mut tab=Tab::new(PathBuf::from("/tmp"));
        tab.entries=(0..8).map(|i|Entry{path:PathBuf::from(format!("/tmp/file{i}")),name:format!("file{i}"),directory:false,symlink:false,size:0,modified:None}).collect();
        tab.select_one(PathBuf::from("/tmp/file1"));
        tab.select_range_to(PathBuf::from("/tmp/file4"));
        assert_eq!(tab.selected_paths.len(),4);
        assert!(tab.selected_paths.contains(Path::new("/tmp/file2")));
        tab.toggle_selection(PathBuf::from("/tmp/file2"));
        assert_eq!(tab.selected_paths.len(),3);
        tab.search="file7".into();tab.select_all_filtered();
        assert_eq!(tab.ordered_selection(),vec![PathBuf::from("/tmp/file7")]);
        tab.clear_selection();assert!(tab.ordered_selection().is_empty());
    }
    #[test] fn batch_copy_continues_after_conflict_without_overwriting() {
        let base=std::env::temp_dir().join(format!("flamewm-explorer-batch-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let source=base.join("source");let dest=base.join("dest");
        fs::create_dir_all(&source).unwrap();fs::create_dir_all(&dest).unwrap();
        fs::write(source.join("a"),"alpha").unwrap();fs::write(source.join("b"),"beta").unwrap();
        fs::write(dest.join("a"),"existing").unwrap();
        let report=transfer_batch(&[source.join("a"),source.join("b")],&dest,false,&mut |_|{});
        assert_eq!((report.completed,report.failed),(2,0));
        assert_eq!(fs::read(dest.join("a")).unwrap(),b"existing");
        assert_eq!(fs::read(dest.join("a (2)")).unwrap(),b"alpha");
        assert_eq!(fs::read(dest.join("b")).unwrap(),b"beta");
        let failing=transfer_batch(&[source.join("missing"),source.join("a")],&dest,true,&mut |_|{});
        assert_eq!((failing.completed,failing.failed),(1,1));
        assert!(failing.first_error.unwrap().contains("missing"));
        assert_eq!(failing.failed_sources,vec![source.join("missing")]);
        assert_eq!(fs::read(dest.join("a (3)")).unwrap(),b"alpha");
        assert!(!source.join("a").exists());
        fs::remove_dir_all(base).unwrap();
    }
    #[test] fn rescan_reconciles_selection_of_deleted_files() {
        let mut model=Explorer::new(PathBuf::from("/tmp"));
        model.tab_mut().select_one(PathBuf::from("/tmp/gone"));
        let generation=model.generation;
        assert!(model.apply_listing(generation,Path::new("/tmp"),Ok(vec![])));
        assert!(model.tab().selected_paths.is_empty());
        assert!(model.tab().selected.is_none());
    }
    #[test] fn drop_policy_moves_on_same_device_copies_cross_device_and_respects_ctrl() {
        assert!(drop_moves(false,100,100));
        assert!(!drop_moves(false,100,200));
        assert!(!drop_moves(true,100,100));
        let base=std::env::temp_dir().join(format!("flamewm-drag-policy-{}-{}",
            std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let source_dir=base.join("source");let dest_dir=base.join("dest");
        fs::create_dir_all(&source_dir).unwrap();fs::create_dir_all(&dest_dir).unwrap();
        let original=source_dir.join("text.txt");fs::write(&original,b"preserve").unwrap();
        let copied=transfer_drop_batch(std::slice::from_ref(&original),&dest_dir,true,&mut |_| {});
        assert_eq!((copied.completed,copied.failed),(1,0));
        assert!(original.exists());assert_eq!(fs::read(dest_dir.join("text.txt")).unwrap(),b"preserve");
        let moved=transfer_drop_batch(std::slice::from_ref(&original),&dest_dir,false,&mut |_| {});
        assert_eq!((moved.completed,moved.failed),(1,0));
        assert!(!original.exists());assert_eq!(fs::read(dest_dir.join("text.txt (2)")).unwrap(),b"preserve");
        fs::remove_dir_all(base).unwrap();
    }

    #[test] fn transfer_rejects_symlink_alias_to_child_before_writing() {
        let base=std::env::temp_dir().join(format!("flamewm-drag-alias-{}-{}",
            std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let source=base.join("source");
        fs::create_dir_all(source.join("child")).unwrap();
        fs::write(source.join("item"), b"original").unwrap();
        symlink(&source.join("child"),base.join("alias")).unwrap();
        let outcome=transfer_batch(&[source.clone()],&base.join("alias"),true,&mut |_| {});
        assert_eq!((outcome.completed,outcome.failed),(0,1));
        assert!(source.join("item").exists());
        assert!(fs::read_dir(source.join("child")).unwrap().next().is_none());
        fs::remove_dir_all(base).unwrap();
    }

    #[test] fn safe_copy_prevents_overwriting_and_recursing_into_itself() {
        let base=std::env::temp_dir().join(format!("flamewm-explorer-test-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(base.join("a")).unwrap();fs::write(base.join("a/item"),"content").unwrap();
        assert!(copy_entry(&base.join("a"),&base.join("a/inside"),&mut |_| {}).is_err());
        copy_entry(&base.join("a/item"),&base.join("copy"),&mut |_|{}).unwrap();
        assert!(copy_entry(&base.join("a/item"),&base.join("copy"),&mut |_|{}).is_err());
        assert_eq!(fs::read(base.join("copy")).unwrap(),b"content");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sidebar_rows_group_places_and_hide_empty_headings() {
        let place = |name: &str, group: PlaceGroup| Place { name: name.into(), path: PathBuf::from("/x"), removable: false, group, icon: "folder", device: None };
        let places = vec![
            place("Home", PlaceGroup::Favorites),
            place("Docs", PlaceGroup::Favorites),
            place("Local Disk", PlaceGroup::Devices),
            place("Work", PlaceGroup::Pinned),
        ];
        assert_eq!(
            sidebar_rows(&places),
            vec![
                SidebarRow::Heading("Favorites"),
                SidebarRow::Place(0),
                SidebarRow::Place(1),
                SidebarRow::Heading("Devices"),
                SidebarRow::Place(2),
                SidebarRow::Heading("Pinned Folders"),
                SidebarRow::Place(3),
            ]
        );
        let without_pins: Vec<Place> = places.iter().filter(|p| p.group != PlaceGroup::Pinned).cloned().collect();
        assert!(!sidebar_rows(&without_pins).contains(&SidebarRow::Heading("Pinned Folders")));
        assert!(sidebar_rows(&[]).is_empty());
    }
}
