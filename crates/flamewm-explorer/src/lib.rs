//! Filesystem and navigation model for the native FlameWM explorer.
//! Filesystem work must be performed by the worker; the UI thread only projects snapshots.
#![cfg_attr(rustfmt, rustfmt::skip)]
use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::os::unix::fs::symlink;
use std::time::SystemTime;

pub mod appicon;
pub mod archive;
pub mod clipboard;
pub mod dndext;
pub mod columns;
pub mod ops;
pub mod places;
pub mod prefs;
pub mod remote;
pub mod properties;
pub mod refresh;
pub mod scroll;
pub mod search;
pub mod thumbs;
pub mod trash;
pub mod view;

#[rustfmt::skip]
pub mod jobs;
/// Row widgets in the template. The list viewport decides how many of them are shown.
pub const ROW_SLOTS: usize = 24;

/// Window row shown by DOM list slot `slot`. The smooth-scroll glide keeps slot 0 as a buffer above
/// the window, so slot k shows window row k-1. Paint and hit-testing both use this mapping.
pub fn row_slot_index(origin: usize, slot: usize) -> isize {
    origin as isize + slot as isize - 1
}
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
    /// `name` lowercased once at construction, so search filters never reallocate it per keystroke.
    lower_name: String,
}
impl Entry {
    pub fn new(path: PathBuf, name: String, directory: bool, symlink: bool, size: u64, modified: Option<SystemTime>) -> Self {
        let lower_name = name.to_lowercase();
        Self { path, name, directory, symlink, size, modified, lower_name }
    }
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
        entries.push(Entry::new(path, name, directory, symlink, meta.len(), meta.modified().ok()));
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

/// Case-insensitive filter match; `query` is already lowercase.
fn entry_matches(entry: &Entry, query: &str) -> bool {
    query.is_empty() || entry.lower_name.contains(query)
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
    /// Bumped by every entries mutation; with the query it keys `filter_cache`.
    revision: u64,
    filter_cache: RefCell<Option<(u64, String, usize)>>,
}
impl Tab {
    pub fn new(path: PathBuf) -> Self { Self { selected_paths:HashSet::new(), history: vec![path], cursor:0, entries:Vec::new(), selected:None, anchor:None, scroll:0, search:String::new(), loading:true, revision:0, filter_cache:RefCell::new(None) } }
    /// Replaces the listing; the filtered-count cache is invalidated.
    pub fn set_entries(&mut self, items: Vec<Entry>) { self.entries=items; self.revision+=1; }
    pub fn extend_entries(&mut self, items: Vec<Entry>) { self.entries.extend(items); self.revision+=1; }
    pub fn clear_entries(&mut self) { self.entries.clear(); self.revision+=1; }
    pub fn path(&self) -> &Path { &self.history[self.cursor] }
    pub fn visit(&mut self, path: PathBuf) {
        self.history.truncate(self.cursor + 1);
        self.history.push(path); self.cursor += 1; self.reset();
    }
    pub fn back(&mut self) -> bool { if self.cursor == 0 { return false; } self.cursor -= 1; self.reset(); true }
    pub fn forward(&mut self) -> bool { if self.cursor + 1 >= self.history.len() { return false; } self.cursor += 1; self.reset(); true }
    pub fn up(&mut self) -> bool {
        if let Some(url)=remote::url_of(self.path()) {
            return match url.parent() { Some(parent) if remote::to_path(&parent)!=self.path() => { self.visit(remote::to_path(&parent)); true }, _ => false };
        }
        if let Some(p)=self.path().parent().map(Path::to_path_buf) { if p != self.path() { self.visit(p); return true; } } false }
    pub fn can_back(&self) -> bool { self.cursor > 0 }
    pub fn can_forward(&self) -> bool { self.cursor + 1 < self.history.len() }
    fn reset(&mut self) { self.clear_entries(); self.selected_paths.clear(); self.selected=None; self.anchor=None; self.scroll=0; self.search.clear(); self.loading=true; }
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
            entry_matches(e, &query)
        }).skip(start.min(target_index)).take(start.abs_diff(target_index)+1)
            .map(|e|e.path.clone());
        if !additive {self.selected_paths.clear();}
        self.selected_paths.extend(range);
        self.selected=Some(target);
    }
    pub fn select_all_filtered(&mut self) {
        self.selected_paths=self.visible().map(|e|e.path.clone()).collect();
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
    /// Entries that pass the search filter, in list order.
    pub fn visible(&self) -> impl Iterator<Item=&Entry> + '_ {
        let query=self.search.to_lowercase();
        self.entries.iter().filter(move |entry| entry_matches(entry, &query))
    }
    /// Filtered rows starting at the scroll offset, at most `rows` of them.
    pub fn visible_window(&self, rows: usize) -> Vec<&Entry> {
        self.visible().skip(self.window_start(rows)).take(rows).collect()
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
        self.visible().nth(index)
    }
    pub fn visible_index_of(&self, path:&Path) -> Option<usize> {
        self.visible().position(|entry| entry.path==path)
    }
    pub fn filtered_count(&self) -> usize {
        let query=self.search.to_lowercase();
        if let Some((revision,cached_query,count))=self.filter_cache.borrow().as_ref() {
            if *revision==self.revision && *cached_query==query {return *count;}
        }
        let count=self.entries.iter().filter(|entry| entry_matches(entry, &query)).count();
        *self.filter_cache.borrow_mut()=Some((self.revision,query,count));
        count
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
    /// Appends one streamed remote chunk when the listing is still the current one.
    pub fn append_listing(&mut self, generation:u64, dir:&Path, items:Vec<Entry>) -> bool {
        if generation!=self.generation || self.tab().path()!=dir { return false; }
        self.tab_mut().extend_entries(items);
        true
    }
    /// Ends a streamed remote listing; false when the listing is stale.
    pub fn finish_listing(&mut self, generation:u64, dir:&Path) -> bool {
        if generation!=self.generation || self.tab().path()!=dir { return false; }
        self.tab_mut().loading=false;
        self.status.clear();
        true
    }
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
                tab.set_entries(items);
            }
            Err(e) => self.status=format!("Cannot read directory: {e}"),
        }
        self.tab_mut().loading=false;
        true
    }
}
/// Sidebar section a place belongs to. Sections render in declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlaceGroup { Places, Devices, Network, Pinned }

impl PlaceGroup {
    pub fn heading(self) -> &'static str {
        match self { Self::Places => "Places", Self::Devices => "Devices", Self::Network => "Network", Self::Pinned => "Pinned Folders" }
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
    for group in [PlaceGroup::Places,PlaceGroup::Devices,PlaceGroup::Network,PlaceGroup::Pinned] {
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
    Place{name:name.into(),path,removable:false,group:PlaceGroup::Places,icon,device:None}
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
    // One mountinfo snapshot feeds both the removable and the network sections.
    let mountinfo=fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    for device in places::parse_mountinfo(&mountinfo) {
        if device.mount_point.is_dir() {
            let name=places::sanitize_label(&device.mount_point.file_name().unwrap_or_default().to_string_lossy());
            // Nameless mounts are not listed, matching Dolphin's sidebar.
            if name.is_empty() {continue;}
            places.push(Place{name,path:device.mount_point,removable:true,group:PlaceGroup::Devices,icon:"usb",device:Some(format!("/dev/{}",device.block_name))});
        }
    }
    places.extend(unmounted.iter().map(unmounted_place));
    // The network group stays hidden when nothing is mounted.
    places.extend(network_places(&mountinfo,&places::gvfs_root()));
    places
}

/// Network locations from mountinfo text: cifs/nfs/sshfs/WebDAV mounts, and each share under the GVFS FUSE root.
pub fn network_places(mountinfo:&str,gvfs_root:&Path)->Vec<Place> {
    let mut out=Vec::new();
    for mount in places::parse_network_mounts(mountinfo) {
        if mount.fstype==places::GVFS_FSTYPE {
            if mount.mount_point==gvfs_root {
                for share in places::gvfs_shares(gvfs_root) {
                    let folder=share.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let name=places::gvfs_label(&folder);
                    if !name.is_empty() {out.push(network_place(name,share));}
                }
            }
            continue;
        }
        if !mount.mount_point.is_dir() {continue;}
        let name=places::sanitize_label(&mount.mount_point.file_name().unwrap_or_default().to_string_lossy());
        if !name.is_empty() {out.push(network_place(name,mount.mount_point));}
    }
    out
}

fn network_place(name:String,path:PathBuf)->Place {
    // No network-specific glyph ships in the template yet, so the drive icon stands in.
    Place{name,path,removable:false,group:PlaceGroup::Network,icon:"drive",device:None}
}

/// Sidebar row for a present but unmounted filesystem: no path, so `mounted()` is false.
/// The drive/removable distinction comes from `lsblk`: an internal partition is listed too, but keeps
/// the internal-drive glyph and is not presented as removable media.
pub fn unmounted_place(device:&places::BlockDevice)->Place {
    let icon=if device.removable {"usb"} else {"drive"};
    Place{name:device.label.clone(),path:PathBuf::new(),removable:device.removable,group:PlaceGroup::Devices,icon,device:Some(device.device.clone())}
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
/// Duplicate target: `name (copy).ext`, then `name (copy 2).ext`, never the source name itself.
pub fn duplicate_destination(from: &Path, directory: &Path) -> PathBuf {
    let name = from.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let (stem, ext) = match from.extension() {
        Some(ext) if from.is_file() => {
            let stem = from.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            (stem, format!(".{}", ext.to_string_lossy()))
        }
        _ => (name, String::new()),
    };
    let first = directory.join(format!("{stem} (copy){ext}"));
    if fs::symlink_metadata(&first).is_err() { return first; }
    for index in 2..=10_000 {
        let target = directory.join(format!("{stem} (copy {index}){ext}"));
        if fs::symlink_metadata(&target).is_err() { return target; }
    }
    directory.join(format!("{stem} (copy overflow){ext}"))
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

/// A drop does nothing when it would not change the file system: the target is one of the dragged items,
/// lies inside one of them (a folder into itself), or is the folder the items already live in (no copy, no "name (1)").
pub fn drop_is_noop(sources:&[PathBuf], target:&Path)->bool {
    sources.iter().any(|source| target==source.as_path() || target.starts_with(source))
        || (!sources.is_empty() && sources.iter().all(|source| source.parent()==Some(target)))
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
    if let Some(url)=remote::url_of(path) {return remote::display_text(&url);}
    if path==home {return "~".into();}
    if let Ok(rest)=path.strip_prefix(home) {return format!("~/{}",rest.display());}
    path.display().to_string()
}
pub enum AddressTarget { Local(PathBuf), Remote(flamewm_remote_fs::Url), ExternalUri(String) }
pub fn parse_address(text:&str, home:&Path)->Result<AddressTarget,String> {
    let trimmed=text.trim();
    if trimmed=="~" {return Ok(AddressTarget::Local(home.to_path_buf()));}
    if let Some(rest)=trimmed.strip_prefix("~/") {return Ok(AddressTarget::Local(home.join(rest)));}
    if trimmed.starts_with("file://") {
        return unescape_uri(trimmed).filter(|p|p.is_absolute())
            .map(AddressTarget::Local).ok_or_else(||"Invalid file URI".to_owned());
    }
    if let Some((scheme,tail))=trimmed.split_once("://") {
        if remote::is_remote_scheme(scheme) {
            return flamewm_remote_fs::Url::parse(trimmed).map(AddressTarget::Remote).map_err(|e|e.to_string());
        }
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
#[rustfmt::skip]
mod tests;

#[cfg(test)]
#[path = "tests/duplicate_naming_tests.rs"]
mod duplicate_naming_tests;

/// Caret and selection for the one active text edit (path bar, search, inline rename, place rename).
/// Indexes are char positions. A selection is replaced by the next typed character.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextField {
    pub text: String,
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
}

impl TextField {
    /// Replaces the text; the caret goes to the end and nothing is selected.
    pub fn set(&mut self, text: String) {
        self.caret = text.chars().count();
        self.text = text;
        self.selection = None;
    }
    /// Replaces the text and selects the char range `select`; the caret sits at its end.
    pub fn set_selected(&mut self, text: String, select: (usize, usize)) {
        self.set(text);
        self.selection = Some(select);
        self.caret = select.1.min(self.len());
    }
    /// Ctrl+A: everything selected, caret at the end.
    pub fn select_all(&mut self) {
        let end = self.len();
        self.selection = Some((0, end));
        self.caret = end;
    }
    pub fn has_selection(&self) -> bool {
        self.selection.is_some_and(|(a, b)| a != b)
    }
    pub fn insert(&mut self, text: &str) {
        self.remove_selection();
        let at = self.byte_at(self.caret);
        self.text.insert_str(at, text);
        self.caret += text.chars().count();
    }
    pub fn backspace(&mut self) {
        if self.remove_selection() || self.caret == 0 {
            return;
        }
        let (from, to) = (self.byte_at(self.caret - 1), self.byte_at(self.caret));
        self.text.replace_range(from..to, "");
        self.caret -= 1;
    }
    pub fn delete(&mut self) {
        if self.remove_selection() {
            return;
        }
        let (from, to) = (self.byte_at(self.caret), self.byte_at(self.caret + 1));
        if from < to {
            self.text.replace_range(from..to, "");
        }
    }
    /// Moves the caret (clamped to the text) and drops any selection.
    pub fn move_caret(&mut self, to: usize) {
        self.selection = None;
        self.caret = to.min(self.len());
    }
    /// The text with a `|` at the caret.
    pub fn display(&self) -> String {
        let at = self.byte_at(self.caret);
        format!("{}|{}", &self.text[..at], &self.text[at..])
    }
    fn len(&self) -> usize {
        self.text.chars().count()
    }
    fn byte_at(&self, char_index: usize) -> usize {
        self.text.char_indices().nth(char_index).map_or(self.text.len(), |(i, _)| i)
    }
    /// Removes the selected range. Returns true when a selection existed.
    fn remove_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection.take() else {
            return false;
        };
        let (lo, hi) = (a.min(b).min(self.len()), a.max(b).min(self.len()));
        let (from, to) = (self.byte_at(lo), self.byte_at(hi));
        self.text.replace_range(from..to, "");
        self.caret = lo;
        true
    }
}

/// Windows-style rename selection in chars: a file selects its stem so typing keeps the extension.
/// Folders, names without an extension and dot-files select everything.
pub fn rename_selection(name: &str, folder: bool) -> (usize, usize) {
    let total = name.chars().count();
    match name.rfind('.') {
        Some(dot) if !folder && dot > 0 => (0, name[..dot].chars().count()),
        _ => (0, total),
    }
}

#[cfg(test)]
#[path = "tests/text_field_tests.rs"]
mod text_field_tests;

#[cfg(test)]
#[path = "tests/filter_cache_tests.rs"]
mod filter_cache_tests;

extern crate self as flamewm_explorer;

/// The application entry point, compiled into this library so the multi-call `flamewm-apps`
/// executable can run it; `src/main.rs` is a one-line wrapper around it.
#[doc(hidden)]
#[rustfmt::skip]
#[path = "main.rs"]
pub mod entry;
