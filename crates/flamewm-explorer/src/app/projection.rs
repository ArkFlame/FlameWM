use super::super::*;

// Projection revision: incremented by every wake, action or key that can change chrome, rows or
// their inputs. A scroll wheel or a window move advances nothing, so a projection triggered only by
// the glide offset repaints the row window and the scrollbar instead of the whole window.
thread_local! {
    static STATE_EPOCH:std::cell::Cell<u64>=const { std::cell::Cell::new(0) };
}
pub(crate) fn note_state_change() {
    STATE_EPOCH.with(|epoch|epoch.set(epoch.get().wrapping_add(1)));
}
fn state_epoch()->u64 {STATE_EPOCH.with(std::cell::Cell::get)}

/// Identity of the last full chrome projection: the revision, the app it belongs to, and the theme
/// colours the literal fills were written with. Equal means nothing outside the row pane changed.
type ChromeKey=(u64,usize,[flamewm_ui_x11::UiColor;4]);
thread_local! {
    static CHROME_KEY:std::cell::Cell<Option<ChromeKey>>=const { std::cell::Cell::new(None) };
}

/// Everything a full row pass renders beyond the entry text: the window it filled, the day its
/// relative dates were built for, the column widths and the literal theme fills. Equal means a new
/// full pass only has to move the rows the last one filled to the current glide offset.
#[derive(Clone,Copy,PartialEq)]
struct RowsKey {
    epoch:u64, origin:usize, rows:usize, first:usize, day:chrono::NaiveDate,
    window_w:f32, name_w:f32, mod_w:f32, size_w:f32, kind_w:f32, kind_shown:bool,
    selected:flamewm_ui_x11::UiColor, clear:flamewm_ui_x11::UiColor,
    alternate:flamewm_ui_x11::UiColor, selection_fill:flamewm_ui_x11::UiColor,
}
thread_local! {
    static ROWS_KEY:std::cell::Cell<Option<RowsKey>>=const { std::cell::Cell::new(None) };
}

/// Tab strip slots compiled into the template (`tab-0`…`tab-4`).
const TAB_SLOTS:usize=5;

/// Context-menu entries, mirroring `MENU_ITEMS`; the per-projection offer pass uses a fixed array.
const MENU_SLOTS:usize=MENU_ITEMS.len();

/// Node ids for one sidebar slot, formatted once instead of on every projection.
struct SidebarIds {
    head:String, head_label:String, prow:String, place:String, place_label:String,
    pact:String, pact_eject:String, pact_mount:String,
    ico:[String; PLACE_ICONS.len()],
}
/// Node ids for one list row slot.
struct SlotIds {
    row:String, name:String, name_dim:String, size:String, modified:String, kind:String,
    ico_thumb:String, ico:[String; ROW_ICONS.len()],
}
/// Node ids for one Open With row.
struct ChooserIds { choice:String, label:String, icon:String }
/// Every template id the projection writes, built once per process. The projection runs on each
/// input and glide frame, so the roughly one thousand `format!("…-{slot}")` calls it made per frame
/// were the largest steady-state allocation source in the explorer window.
struct IdTables {
    sidebar:Vec<SidebarIds>, rows:Vec<SlotIds>, tabs:Vec<String>, menus:Vec<String>,
    col_sep:[String;3], col_grip:[String;3], sort_asc:[String;4], sort_desc:[String;4],
    crumbs:Vec<String>, dialog_lines:[String;4], dialog_buttons:[String;4],
    chooser:Vec<ChooserIds>,
}
thread_local! {
    static ID_TABLES:RefCell<Option<Rc<IdTables>>>=const { RefCell::new(None) };
}
impl IdTables {
    fn get()->Rc<IdTables> {
        ID_TABLES.with(|cell|Rc::clone(cell.borrow_mut().get_or_insert_with(||Rc::new(Self::build()))))
    }
    fn build()->Self {
        Self {
            sidebar:(0..SIDEBAR_SLOTS).map(|slot|SidebarIds{
                head:format!("sb-head-{slot}"),
                head_label:format!("sb-head-label-{slot}"),
                prow:format!("prow-{slot}"),
                place:format!("place-{slot}"),
                place_label:format!("place-label-{slot}"),
                pact:format!("pact-{slot}"),
                pact_eject:format!("pact-eject-{slot}"),
                pact_mount:format!("pact-mount-{slot}"),
                ico:PLACE_ICONS.map(|kind|format!("pico-{slot}-{kind}")),
            }).collect(),
            rows:(0..ROW_SLOTS).map(|slot|SlotIds{
                row:format!("row-{slot}"),
                name:format!("name-{slot}"),
                name_dim:format!("name-dim-{slot}"),
                size:format!("size-{slot}"),
                modified:format!("modified-{slot}"),
                kind:format!("kind-{slot}"),
                ico_thumb:format!("ico-{slot}-thumb"),
                ico:ROW_ICONS.map(|kind|format!("ico-{slot}-{kind}")),
            }).collect(),
            tabs:(0..TAB_SLOTS).map(|i|format!("tab-{i}")).collect(),
            menus:MENU_ITEMS.iter().map(|item|format!("menu-{item}")).collect(),
            col_sep:["mod","size","kind"].map(|key|format!("col-sep-{key}")),
            col_grip:["mod","size","kind"].map(|key|format!("col-grip-{key}")),
            sort_asc:["name","mod","size","kind"].map(|key|format!("sort-{key}-asc")),
            sort_desc:["name","mod","size","kind"].map(|key|format!("sort-{key}-desc")),
            crumbs:(0..BREADCRUMB_SLOTS).map(|i|format!("breadcrumb-{i}")).collect(),
            dialog_lines:std::array::from_fn(|i|format!("dialog-line-{i}")),
            dialog_buttons:std::array::from_fn(|i|format!("dialog-b{}",i+1)),
            chooser:(0..CHOICE_ROWS).map(|slot|ChooserIds{
                choice:format!("openwith-choice-{slot}"),
                label:format!("openwith-label-{slot}"),
                icon:format!("openwith-icon-{slot}"),
            }).collect(),
        }
    }
}

// `HOME` resolved once: `home_dir` stats the candidate, and the address bar asks on every projection.
thread_local! {
    static HOME_DIR:std::cell::OnceCell<PathBuf>=const { std::cell::OnceCell::new() };
}
fn display_home(path:&Path)->String {
    HOME_DIR.with(|cell|display_location(path,cell.get_or_init(home_dir)))
}

// Address bar text for the last folder: `display_location` formats a new string per call and the
// bar is re-projected on every wake, so the same folder reuses its text.
thread_local! {
    static ADDRESS_TEXT:RefCell<Option<(PathBuf,Rc<String>)>>=const { RefCell::new(None) };
}
fn cached_address(path:&Path)->Rc<String> {
    ADDRESS_TEXT.with(|cell|{
        let mut cell=cell.borrow_mut();
        if let Some((cached,text))=cell.as_ref() && cached.as_path()==path {return Rc::clone(text);}
        let text=Rc::new(display_home(path));
        *cell=Some((path.to_path_buf(),Rc::clone(&text)));
        text
    })
}

// Tab titles, kept per tab path so a projection does not rebuild them from `file_name` (which
// allocates through `to_string_lossy`) for every tab on every wake.
thread_local! {
    static TAB_TITLES:RefCell<Vec<(PathBuf,Rc<String>)>>=const { RefCell::new(Vec::new()) };
}

// Item count for the status strip, rebuilt only when the counts change (text_ref then skips the copy).
thread_local! {
    #[allow(clippy::type_complexity)]
    static COUNT_TEXT:RefCell<Option<((usize,usize),Rc<String>)>>=const { RefCell::new(None) };
}
fn cached_count(total:usize,selected:usize)->Rc<String> {
    COUNT_TEXT.with(|cell|{
        let mut cell=cell.borrow_mut();
        if let Some((cached,text))=cell.as_ref() && *cached==(total,selected) {return Rc::clone(text);}
        let text=Rc::new(view::items_text(total,selected));
        *cell=Some(((total,selected),Rc::clone(&text)));
        text
    })
}
// Free-space label, rebuilt only when the statvfs sample changes.
thread_local! {
    static FREE_TEXT:RefCell<Option<(Option<u64>,Rc<String>)>>=const { RefCell::new(None) };
}
fn cached_free(free:Option<u64>)->Rc<String> {
    FREE_TEXT.with(|cell|{
        let mut cell=cell.borrow_mut();
        if let Some((cached,text))=cell.as_ref() && *cached==free {return Rc::clone(text);}
        let text=Rc::new(free.map_or_else(String::new,|bytes|format!("{} left",flamewm_api::format::human_size(bytes))));
        *cell=Some((free,Rc::clone(&text)));
        text
    })
}

/// Progress rows depend on one snapshot. The snapshot is compared, not copied, so a transfer that is
/// not advancing costs no string formatting on the exploration window's frames.
struct ProgressLabels { label:String, percent:String, current:String, bytes:String }
thread_local! {
    static PROGRESS_TEXT:RefCell<Option<(Option<progress::Snapshot>,Rc<ProgressLabels>)>>=const { RefCell::new(None) };
}
fn cached_progress(snapshot:Option<&progress::Snapshot>)->Rc<ProgressLabels> {
    PROGRESS_TEXT.with(|cell|{
        let mut cell=cell.borrow_mut();
        if let Some((cached,text))=cell.as_ref() && cached.as_ref()==snapshot {return Rc::clone(text);}
        let text=Rc::new(match snapshot {
            Some(snapshot)=>ProgressLabels{
                label:snapshot.label(),
                percent:format!("{}%",snapshot.percent()),
                current:snapshot.current.clone().unwrap_or_else(||"Starting…".into()),
                bytes:snapshot.bytes_text(),
            },
            None=>ProgressLabels{label:String::new(),percent:String::new(),current:"Starting…".into(),bytes:String::new()},
        });
        *cell=Some((snapshot.cloned(),Rc::clone(&text)));
        text
    })
}

// Reused sidebar row buffer: `sidebar_rows` allocated a result vector plus one index vector per
// group on every projection.
thread_local! {
    static SIDEBAR_ROWS:RefCell<Vec<SidebarRow>>=const { RefCell::new(Vec::new()) };
}

/// Identity of the thumbnail requests one full pass makes: the listing generation and the first
/// visible row. A projection that shows the same window re-requests exactly the rows that are already
/// queued, so the queue is left alone instead of cloning and re-hashing every waiting key.
struct ThumbPassKey { path:PathBuf, revision:u64, first:usize, rows:usize }
thread_local! {
    static THUMB_PASS:RefCell<Option<ThumbPassKey>>=const { RefCell::new(None) };
}
/// `true` when this pass shows a different window than the last one and must restart the queue.
fn thumb_pass_changed(tab:&flamewm_explorer::Tab,first:usize,rows:usize)->bool {
    THUMB_PASS.with(|cell|{
        let mut cell=cell.borrow_mut();
        if let Some(key)=cell.as_ref()
            && key.path==tab.path() && key.revision==tab.revision() && key.first==first && key.rows==rows
        {return false;}
        *cell=Some(ThumbPassKey{path:tab.path().to_path_buf(),revision:tab.revision(),first,rows});
        true
    })
}

// Breadcrumb labels depend only on the current path: rebuilding them on every projection walked all
// components and allocated one string plus one ancestor path per crumb.
thread_local! {
    #[allow(clippy::type_complexity)]
    static CRUMBS:RefCell<Option<(PathBuf,Rc<Vec<(String,PathBuf)>>)>>=const { RefCell::new(None) };
}
fn breadcrumbs_cached(path:&Path)->Rc<Vec<(String,PathBuf)>> {
    CRUMBS.with(|cell|{
        let mut cell=cell.borrow_mut();
        if let Some((cached,crumbs))=cell.as_ref() && cached.as_path()==path {return Rc::clone(crumbs);}
        let crumbs=Rc::new(breadcrumbs(path));
        *cell=Some((path.to_path_buf(),Rc::clone(&crumbs)));
        crumbs
    })
}

/// Row text depends only on the entry itself, so it is formatted when the entry at a slot changes
/// instead of on every frame. `day` keeps the relative "Today"/"Yesterday" label correct at midnight.
struct LabelKey {
    path:PathBuf, size:u64, modified:Option<std::time::SystemTime>, directory:bool, symlink:bool, day:chrono::NaiveDate,
}
/// Everything a row shows that derives only from its entry: the three labels, the icon family, and the
/// thumbnail key. `file_icon` lowercases the extension and `ThumbKey::of` clones the path, so building
/// them per frame cost two allocations per visible row on every full projection.
struct RowLabels { size:String, modified:String, kind:String, icon:&'static str, thumb:Option<ThumbKey> }
thread_local! {
    static ROW_LABELS:RefCell<Vec<Option<(LabelKey,RowLabels)>>>=const { RefCell::new(Vec::new()) };
    /// Which slots held an entry in the last full projection. A glide frame moves those rows and touches
    /// nothing else, so it no longer re-filters the listing up to the scroll origin to rebuild them.
    static ROW_OCCUPANCY:RefCell<[bool;ROW_SLOTS]>=const { RefCell::new([false;ROW_SLOTS]) };
    /// Type-icon family each slot currently shows. `show_type_icon` hides every other family before it
    /// shows one; skipping that pass when the family is unchanged removes `ROW_ICONS.len()` document
    /// lookups per visible row per full projection, the largest per-row cost left in a scroll frame.
    static TYPE_ICON_SHOWN:RefCell<[Option<&'static str>;ROW_SLOTS]>=const { RefCell::new([None;ROW_SLOTS]) };
}
// Local date, refreshed at most once per second: `Local::now` resolves the time zone and the date is
// only used to invalidate "Today"/"Yesterday" labels.
thread_local! {
    static TODAY:RefCell<Option<(Instant,chrono::NaiveDate)>>=const { RefCell::new(None) };
}
fn today_cached()->chrono::NaiveDate {
    TODAY.with(|cell|{
        let mut cell=cell.borrow_mut();
        let now=Instant::now();
        if let Some((taken,day))=cell.as_ref() && now.duration_since(*taken)<Duration::from_secs(1) {return *day;}
        let day=chrono::Local::now().date_naive();
        *cell=Some((now,day));
        day
    })
}

/// Labels for row `slot`, rebuilt only when its entry or the local date changes.
fn row_labels<'a>(cache:&'a mut Vec<Option<(LabelKey,RowLabels)>>,slot:usize,entry:&Entry,day:chrono::NaiveDate)->&'a RowLabels {
    if cache.len()<=slot {cache.resize_with(slot+1,||None);}
    let stale=match cache[slot].as_ref() {
        Some((key,_))=>key.path!=entry.path||key.size!=entry.size||key.modified!=entry.modified
            ||key.directory!=entry.directory||key.symlink!=entry.symlink||key.day!=day,
        None=>true,
    };
    if stale {
        cache[slot]=Some((LabelKey{
                path:entry.path.clone(), size:entry.size, modified:entry.modified,
                directory:entry.directory, symlink:entry.symlink, day,
            },RowLabels{
                size:entry.size_label(),
                modified:entry.modified_label(),
                kind:view::kind_name(entry),
                icon:view::file_icon(entry),
                // The key carries a path clone and a normalised extension; remote paths never thumbnail.
                thumb:(!remote::is_remote(&entry.path)).then(||ThumbKey::of(entry)).flatten(),
            }));
    }
    cache[slot].as_ref().map_or(&EMPTY_LABELS,|(_,labels)|labels)
}
/// Fallback for a slot the fill above did not produce (it always does): keeps the projection free of a
/// panic path if that invariant ever breaks.
static EMPTY_LABELS:RowLabels=RowLabels{
    size:String::new(), modified:String::new(), kind:String::new(), icon:"file", thumb:None,
};

/// What decides the context-menu offer. Building it is cheap; the flags it guards are not
/// (`offered_formats` walks `PATH` and stats each candidate), and an open menu re-projects on every wake.
struct MenuKey {
    context_place:Option<usize>, context_on_entry:bool, selected:Option<PathBuf>,
    many:bool, in_trash:bool, searching:bool, overflow:bool, ark:bool, places_len:usize,
    /// Kind and state of the sidebar place the menu targets: mount and bookmark offers depend on them.
    place_state:Option<(bool,bool,PlaceGroup)>,
}
thread_local! {
    static MENU_ENABLED:RefCell<Option<(MenuKey,Rc<Vec<bool>>)>>=const { RefCell::new(None) };
}

impl ExplorerApp {
    /// The rubber-band overlay is the only node a rubber drag changes before release.
    pub(crate) fn project_rubber(&self, doc:&mut impl UiDocumentAccess)->Result<(),String>{
        if let Some(((x0,y0),(x1,y1)))=self.rubber {
            let width=(x1-x0).abs();let height=(y1-y0).abs();
            let visible=width>5.0 || height>5.0;
            doc.visible("selection-rectangle",visible)?;
            if visible {
                doc.position("selection-rectangle",x0.min(x1),y0.min(y1))?;
                doc.size("selection-rectangle",width.max(1.0),height.max(1.0))?;
            }
        } else {doc.visible("selection-rectangle",false)?;}
        Ok(())
    }

    /// Whether each `MENU_ITEMS` entry is offered for the current context, cached until that context
    /// changes. `offered_formats` walks `PATH` with a stat per directory; recomputing the flags on every
    /// wake while a menu is open put filesystem work on the UI thread per frame.
    fn menu_enabled(&self,selected:Option<&Path>,many:bool,in_trash:bool,searching:bool)->Rc<Vec<bool>> {
        let place_state=self.context_place.and_then(|i|self.places.get(i)).map(|p|(p.removable,p.mounted(),p.group));
        MENU_ENABLED.with(|cell|{
            let mut cell=cell.borrow_mut();
            let stale=match cell.as_ref() {
                Some((cached,_))=>cached.context_place!=self.context_place||cached.context_on_entry!=self.context_on_entry
                    ||cached.selected.as_deref()!=selected||cached.many!=many||cached.in_trash!=in_trash
                    ||cached.searching!=searching||cached.overflow!=self.menu_overflow||cached.ark!=self.ark
                    ||cached.places_len!=self.places.len()||cached.place_state!=place_state,
                None=>true,
            };
            if stale {
                let flags=MENU_ITEMS.iter().map(|item|self.menu_item_enabled(item,selected,many,in_trash,searching)).collect::<Vec<_>>();
                *cell=Some((MenuKey{
                    context_place:self.context_place, context_on_entry:self.context_on_entry,
                    selected:selected.map(Path::to_path_buf), many, in_trash, searching,
                    overflow:self.menu_overflow, ark:self.ark, places_len:self.places.len(), place_state,
                },Rc::new(flags)));
            }
            match cell.as_ref() {
                Some((_,flags))=>Rc::clone(flags),
                // The fill above always stores a state; keep the projection panic-free if it ever did not.
                None=>Rc::new(Vec::new()),
            }
        })
    }
    /// One context-menu entry's offer state; the arms cover the sidebar, entry and background menus.
    fn menu_item_enabled(&self,item:&str,selected:Option<&Path>,many:bool,in_trash:bool,searching:bool)->bool {
        if self.context_place.is_some() {
            // Sidebar menu: bookmarks can be renamed or removed, mounted devices unmounted or ejected.
            match item {
                "placerename"|"placeremove"=>self.context_place.is_some_and(|i|self.is_bookmark(i)),
                "unmount"|"eject"=>self.context_place.is_some_and(|i|self.places.get(i).is_some_and(|p|p.removable && p.mounted())),
                "open"|"newtab"|"properties"=>self.context_place.is_some(),
                "emptytrash"=>self.context_place.and_then(|i|self.places.get(i)).is_some_and(|place|self.is_trash_files(&place.path)),
                _=>false,
            }
        } else {match item {
            // Folder-only actions: a file entry offers none; a folder entry offers its own; the background offers the current folder.
            "newfolder"=>!self.context_on_entry,
            "terminal"|"bookmark"|"newtab"=>!self.context_on_entry || self.selected_folder().is_some(),
            "paste"=>true,
            "emptytrash"=>in_trash,
            "extract"=>self.ark && self.context_on_entry && !many && selected.is_some_and(is_archive),
            "compress"=>self.ark && self.context_on_entry && selected.is_some(),
            "copy"|"cut"|"trash"|"duplicate"=>self.context_on_entry && selected.is_some(),
            "open"|"openwith"|"rename"|"properties"=>self.context_on_entry && !many && selected.is_some(),
            // Offered only for a local regular file whose format has at least one conversion target.
            "convert"=>self.context_on_entry && !many && selected.is_some_and(|p|!remote::is_remote(p) && p.is_file() && !flamewm_conversion_core::conversion::offered_formats(p).is_empty()),
            "setwallpaper"=>self.context_on_entry && !many && selected.is_some_and(|p|!remote::is_remote(p) && p.is_file() && thumbs::thumb_format(p).is_some_and(crate::thumbs::ThumbFormat::is_image)),
            "opencontaining"=>searching && self.context_on_entry && !many && selected.is_some(),
            "restore"=>in_trash && self.context_on_entry && selected.is_some(),
            "hidden"|"showtype"|"connect"=>self.menu_overflow,
            _=>false,
        }}
    }

    pub(crate) fn project(&self, doc:&mut impl UiDocumentAccess)->Result<(),String>{
        let chrome=(state_epoch(),std::ptr::from_ref(self) as usize,self.chrome_colors());
        // Only a running glide projects the same window from a scroll-derived wake; every other
        // projection may carry a state change the revision already records, so keep it complete.
        if self.glide.is_animating(Instant::now()) && CHROME_KEY.with(std::cell::Cell::get)==Some(chrome) {
            // A wheel notch, a scrollbar drag or a glide frame only moves the row window. Repaint the
            // rows and the thumb; the sides, toolbar, tabs, status strip and popups are already right.
            self.theme.project(doc)?;
            self.project_rows(doc,true)?;
            return self.project_scroll(doc);
        }
        self.theme.project(doc)?;
        let ids=IdTables::get();
        let tab=self.model.tab();
        // The path bar always shows the folder; search text belongs to the search field only.
        match self.edit {
            Some(EditMode::Path)=>doc.text_ref("address-text",&self.field.display())?,
            Some(EditMode::Mode)=>doc.text_ref("address-text",&format!("Permissions: {}|",self.field.text))?,
            _=>{let address=cached_address(tab.path()); doc.text_ref("address-text",address.as_str())?;}
        }
        let crumbs:Option<Rc<Vec<(String,PathBuf)>>>=if self.edit.is_none() {Some(breadcrumbs_cached(tab.path()))} else {None};
        for (slot,id) in ids.crumbs.iter().enumerate().take(BREADCRUMB_SLOTS) {
            let crumb=crumbs.as_ref().and_then(|crumbs|crumbs.get(slot));
            doc.visible(id,crumb.is_some())?;
            if let Some((label,_))=crumb {doc.text_ref(id,label)?;}
        }
        doc.visible("address-text",crumbs.as_ref().is_none_or(|crumbs|crumbs.is_empty()))?;
        doc.background("address-text",if matches!(self.edit,Some(EditMode::Path)) && self.field.has_selection() {self.theme.selection_fill()} else {self.theme.clear()})?;
        // Toolbar fields have fixed widths (the layout engine cannot shrink them): drop the path bar first, then search, so they never overlap.
        doc.visible("address",self.window.0>=480.0)?;
        doc.visible("search-toggle",self.window.0>=400.0)?;
        // The field shows only the query: a plain placeholder when empty, and the clear button while a search is active.
        match self.edit {
            Some(EditMode::Search)=>doc.text_ref("search-text",&self.field.display())?,
            _ if self.search_rows.is_some()=>doc.text_ref("search-text",&self.search_text)?,
            _=>doc.text_ref("search-text","Search")?,
        }
        doc.background("search-text",if matches!(self.edit,Some(EditMode::Search)) && self.field.has_selection() {self.theme.selection_fill()} else {self.theme.clear()})?;
        doc.visible("search-clear",self.search_rows.is_some())?;
        doc.text_ref("menu-connect-label","Connect to Server…")?;
        doc.text_ref("menu-hidden-label",if self.model.hidden {"Show Hidden Files: On"} else {"Show Hidden Files: Off"})?;
        doc.text_ref("menu-showtype-label",if self.show_type {"Show Type Column: On"} else {"Show Type Column: Off"})?;
        doc.foreground("nav-back",if tab.can_back(){self.theme.text()}else{self.theme.muted()})?;
        doc.foreground("nav-forward",if tab.can_forward(){self.theme.text()}else{self.theme.muted()})?;
        // Native Finder-style chrome: a single tab has no visible tab strip.
        let tab_strip=self.model.tabs.len()>1;
        doc.visible("tabs",tab_strip)?;
        if tab_strip {
            TAB_TITLES.with(|cell|->Result<(),String>{
                let mut titles=cell.borrow_mut();
                titles.truncate(self.model.tabs.len());
                for (i,tab) in self.model.tabs.iter().enumerate() {
                    if titles.get(i).is_some_and(|(cached,_)|cached.as_path()==tab.path()) {continue;}
                    let name=tab.path().file_name().unwrap_or_default().to_string_lossy();
                    let title=Rc::new(if name.is_empty(){"System".to_string()}else{name.into_owned()});
                    if let Some(slot)=titles.get_mut(i) {*slot=(tab.path().to_path_buf(),title);}
                    else {titles.push((tab.path().to_path_buf(),title));}
                }
                for (i,id) in ids.tabs.iter().enumerate() {
                    let exists=i<self.model.tabs.len();
                    doc.visible(id,exists)?;
                    if let Some((_,title))=titles.get(i) {
                        doc.text_ref(id,title.as_str())?;
                        doc.background(id,if i==self.model.active{self.theme.selected()}else{self.theme.card()})?;
                    }
                }
                Ok(())
            })?;
        } else {
            for id in &ids.tabs {doc.visible(id,false)?;}
        }
        // One reused buffer: the row list depends only on the places, and the projection runs on
        // every wake, so it no longer allocates a result vector plus one index vector per group.
        SIDEBAR_ROWS.with(|cell|->Result<(),String>{
        let mut rows=cell.borrow_mut();
        fill_sidebar_rows(&mut rows,&self.places);
        for (slot,slot_ids) in ids.sidebar.iter().enumerate() {
            // Each slot shows either a group heading or a place button; the other element stays hidden.
            match rows.get(slot) {
                Some(SidebarRow::Heading(title)) => {
                    doc.visible(&slot_ids.head,true)?;
                    doc.text_ref(&slot_ids.head_label,title)?;
                    doc.visible(&slot_ids.prow,false)?;
                    doc.visible(&slot_ids.place,false)?;
                }
                Some(SidebarRow::Place(index)) => {
                    let place=&self.places[*index];
                    let selected=!place.path.as_os_str().is_empty() && place.path==tab.path();
                    let unmounted=!place.mounted();
                    doc.visible(&slot_ids.head,false)?;
                    doc.visible(&slot_ids.prow,true)?;
                    doc.visible(&slot_ids.place,true)?;
                    for (kind,icon_id) in PLACE_ICONS.iter().zip(&slot_ids.ico) {doc.visible(icon_id,*kind==place.icon)?;}
                    // Inline place rename: the label becomes the field.
                    if matches!(self.edit,Some(EditMode::PlaceName)) && self.rename_target.as_ref()==Some(&place.path) {
                        doc.text_ref(&slot_ids.place_label,&self.field.display())?;
                    } else {
                        doc.text_ref(&slot_ids.place_label,&place.name)?;
                    }
                    doc.foreground(&slot_ids.place_label,if unmounted && place.device.is_some() {self.theme.muted()} else {self.theme.text()})?;
                    doc.background(&slot_ids.place,if selected {self.theme.selected()} else {self.theme.clear()})?;
                    // Device rows get a Dolphin-style button: eject while mounted, mount while not.
                    let device=place.device.is_some();
                    doc.visible(&slot_ids.pact,device)?;
                    doc.visible(&slot_ids.pact_eject,device && place.mounted())?;
                    doc.visible(&slot_ids.pact_mount,device && unmounted)?;
                }
                None => {
                    doc.visible(&slot_ids.head,false)?;
                    doc.visible(&slot_ids.prow,false)?;
                    doc.visible(&slot_ids.place,false)?;
                }
            }
        }
        Ok(())
        })?;
        // Only Details shows the metadata columns; Icons shows names alone.
        // The header and every row share one geometry: flow cells with identical fixed widths.
        doc.position("columns",SIDEBAR_WIDTH,header_top(tab_strip))?;
        doc.size("columns",self.window.0-SIDEBAR_WIDTH,HEADER_HEIGHT)?;
        doc.visible("columns",true)?;
        // Name fills the space the fixed columns leave. Each divider sits on the left edge of its fixed column.
        let total=self.window.0-SIDEBAR_WIDTH;
        let kind_shown=self.show_type;
        let name_w=self.widths.name_width(total,kind_shown);
        let mod_x=name_w;let size_x=mod_x+self.widths.modified;let kind_x=size_x+self.widths.size;
        doc.size("col-name",name_w,HEADER_HEIGHT)?;
        doc.size("col-mod",self.widths.modified,HEADER_HEIGHT)?;
        doc.size("col-size",self.widths.size,HEADER_HEIGHT)?;
        doc.size("col-kind",self.widths.kind,HEADER_HEIGHT)?;
        doc.visible("col-kind",kind_shown)?;
        for (i,(x,shown)) in [(mod_x,true),(size_x,true),(kind_x,kind_shown)].into_iter().enumerate() {
            doc.position(&ids.col_sep[i],x,0.0)?;
            doc.visible(&ids.col_sep[i],shown)?;
            doc.position(&ids.col_grip[i],x-COLUMN_GRIP_HALF,0.0)?;
            doc.visible(&ids.col_grip[i],shown)?;
        }
        // The engine has no flex-shrink, so a tall sidebar would push the status strip out of the window.
        // The body gets an explicit height: window minus toolbar/tabs, the two 1px hairlines and the status strip.
        // The root follows the settled window size too: a stale root height would keep the status strip below the window.
        doc.size("explorer",self.window.0,self.window.1)?;
        let body_h=(self.window.1-header_top(tab_strip)-2.0-STATUS_STRIP_HEIGHT).max(0.0);
        doc.size("body-area",self.window.0,body_h)?;
        // The sidebar is a row child that would otherwise keep its content height and paint over the status strip.
        doc.size("places",SIDEBAR_WIDTH,body_h)?;
        // Sort direction is a drawn triangle per column; only the active column shows one.
        for (i,column) in [Column::Name,Column::Modified,Column::Size,Column::Type].into_iter().enumerate() {
            let active=self.column_sort.column==column;
            doc.visible(&ids.sort_asc[i],active&&!self.column_sort.descending)?;
            doc.visible(&ids.sort_desc[i],active&&self.column_sort.descending)?;
        }
        self.project_rows(doc,true)?;
        self.project_scroll(doc)?;
        self.project_rubber(doc)?;
        // Empty list space under the rows is the background context surface (rubber band and background menu).
        doc.visible("list-surface",true)?;
        let surface_top=list_top(self.model.tabs.len()>1);
        let surface_bottom=self.window.1-STATUS_STRIP_HEIGHT;
        doc.position("list-surface",SIDEBAR_WIDTH,surface_top)?;
        doc.size("list-surface",(self.window.0-SIDEBAR_WIDTH).max(1.0),(surface_bottom-surface_top).max(1.0))?;
        // Drag ghost: the dragged file's name and type icon at half opacity, just below the pointer.
        let ghost=self.row_drag.as_ref().filter(|drag|drag.dragging);
        doc.visible("drag-ghost",ghost.is_some())?;
        if let Some(drag)=ghost {
            let folder=drag.source.is_dir();
            doc.text("drag-ghost-label",drag.source.file_name().unwrap_or_default().to_string_lossy().into_owned())?;
            doc.visible("drag-ghost-folder",folder)?;
            doc.visible("drag-ghost-file",!folder)?;
            doc.position("drag-ghost",drag.pointer.0-12.0,drag.pointer.1+16.0)?;
        }
        let drag_status;
        let status:&str=if self.row_drag.as_ref().is_some_and(|drag|drag.dragging) {
            if let Some(folder)=self.drop_folder.as_deref() {
                drag_status=format!("Drop into {} · Move (Ctrl to copy)",folder.file_name().unwrap_or_default().to_string_lossy());
                &drag_status
            } else {"Drag onto a folder to move · Hold Ctrl to copy"}
        } else {self.activity.as_deref().unwrap_or(self.model.status.as_str())};
        doc.text_ref("status-message",status)?;
        let count=cached_count(tab.entries.len(),tab.selected_paths.len());
        doc.text_ref("status-count",count.as_str())?;
        // Free space and usage come from an off-thread statvfs sample (see `space_for`); the bar
        // fills by the share of the filesystem in use. Nothing here touches the filesystem.
        let space=self.space_for(tab.path());
        let free=cached_free(space.map(|(free,_)|free));
        doc.text_ref("status-free",free.as_str())?;
        let used=space.map_or(0.0,|(free,total)|if total>0 {1.0-free as f32/total as f32} else {0.0});
        doc.size("usage-fill",USAGE_TRACK_WIDTH*used.clamp(0.0,1.0),4.0)?;
        // The progress window and the activity entry render one shared snapshot, so
        // the strip always agrees with the window. Nothing here touches the filesystem.
        let snapshot=self.progress.as_ref();
        let job=cached_progress(snapshot);
        doc.text_ref("job-progress",&job.label)?;
        doc.visible("progress-popup",snapshot.is_some())?;
        let fraction=snapshot.map_or(0.0,|s|f32::from(s.percent())/100.0);
        doc.text_ref("progress-percent",&job.percent)?;
        doc.text_ref("progress-current",&job.current)?;
        doc.text_ref("progress-bytes",&job.bytes)?;
        doc.size("progress-fill",PROGRESS_TRACK_WIDTH*fraction,8.0)?;
        doc.visible("context-popup",self.context.is_some() && !self.chooser_loading && self.chooser.is_none())?;
        self.project_chooser(doc)?;
        self.project_convert_picker(doc)?;
        self.project_dialog(doc)?;
        // A closed menu's rows are invisible behind a hidden popup. Skipping the offer pass and its
        // `MENU_ITEMS` visibility writes keeps scrolling away from probing/formatting menu state; an
        // opening menu changes the revision, so this pass always runs before its rows are shown.
        if let Some((_x,_y))=self.context {
            let selected_path=self.selected();
            let many=self.model.tab().selected_paths.len()>1;
            let in_trash=self.is_trash_files(tab.path());
            let searching=self.search_rows.is_some();
            let enabled=self.menu_enabled(selected_path.as_deref(),many,in_trash,searching);
            // Offered indices, in menu order. A fixed array keeps this pass allocation-free: the menu
            // can stay open across frames, and a Vec allocated on every projection.
            let mut shown=[0usize;MENU_SLOTS];
            let mut shown_len=0;
            for (index,menu) in ids.menus.iter().enumerate().take(MENU_ITEMS.len()) {
                let offered=enabled.get(index).copied().unwrap_or(false);
                doc.visible(menu,offered)?;
                if offered {shown[shown_len]=index;shown_len+=1;}
            }
            let (x,y)=self.context.unwrap_or_default();
            // Shared desktop context-menu metrics; hidden actions consume no space.
            let metrics=flamewm_ui_core::context_menu::MenuMetrics::canonical();
            let row_h=metrics.row_height as f32;
            let pad=metrics.padding as f32;
            // Rows that fit the window; a taller menu shows a window-sized slice that the wheel moves through.
            let fit=(((self.window.1-2.0*pad)/row_h).floor().max(0.0) as usize).min(shown_len);
            let first=self.context_scroll.min(shown_len-fit);
            for (index,&position) in shown[..shown_len].iter().enumerate() {
                doc.visible(&ids.menus[position],index>=first && index<first+fit)?;
            }
            let height=fit as f32*row_h+2.0*pad;
            // The overflow menu hangs from the right edge of its button; other menus open at the pointer.
            let left=if self.menu_overflow {x-210.0} else {x};
            doc.position("context-popup",left.min(self.window.0-210.0).max(0.0),y.min(self.window.1-height).max(0.0))?;
        }
        CHROME_KEY.with(|key|key.set(Some(chrome)));
        Ok(())
    }

    /// Literal row fills derived from the theme. A settings snapshot changes these without changing the
    /// projection revision, so they are part of both projection keys.
    fn chrome_colors(&self)->[flamewm_ui_x11::UiColor;4] {
        [self.theme.selected(),self.theme.clear(),self.theme.row_alternate(),self.theme.selection_fill()]
    }

    /// Scrollbar geometry: drawn only while some entries are outside the window; thumb size and position
    /// follow the window. Kept separate so a rows-only projection still updates the thumb.
    fn project_scroll(&self, doc:&mut impl UiDocumentAccess)->Result<(),String> {
        let rows=self.visible_rows();
        let origin=self.origin(rows);
        let (track_top,track_h)=self.scrollbar_track();
        let track_x=self.window.0-SCROLLBAR_TRACK_WIDTH;
        if let Some((thumb_top,thumb_len)) = scroll::thumb_geometry(track_h,self.scroll_total(),rows,origin) {
            doc.position("scroll-track",track_x,track_top)?;
            doc.size("scroll-track",SCROLLBAR_TRACK_WIDTH,track_h)?;
            doc.visible("scroll-track",true)?;
            doc.position("scroll-thumb",track_x+SCROLLBAR_THUMB_INSET,track_top+thumb_top)?;
            doc.size("scroll-thumb",SCROLLBAR_THUMB_WIDTH,thumb_len)?;
            doc.visible("scroll-thumb",true)?;
        } else {
            doc.visible("scroll-track",false)?;
            doc.visible("scroll-thumb",false)?;
        }
        Ok(())
    }

    /// One shared modal: four text lines and four fixed-slot buttons, labelled per dialog kind.
    /// Shows the row's decoded thumbnail, or the type icon until it is ready. A slot is rebound only when its image id changes.
    /// List rows glide: slot k draws window row k-1 at its eased offset; the outer slots are off-window until they settle.
    /// A glide frame (`full == false`) only moves rows and toggles their visibility: the origin is unchanged, so each
    /// slot keeps the text, icon and background of the last full projection.
    pub(crate) fn project_rows(&self, doc:&mut impl UiDocumentAccess, full:bool)->Result<(),String> {
        let top=list_top(self.model.tabs.len()>1);
        let bottom=self.window.1-STATUS_STRIP_HEIGHT;
        let glide=self.glide.offset(Instant::now());
        // A glide frame moves only the rows the last full pass filled: their text, icons and backgrounds
        // are already right, so the listing is not filtered again up to the scroll origin or even walked.
        if !full {return self.project_rows_positions(doc);}
        let tab=self.model.tab();
        let kind_shown=self.show_type;
        let rows=self.visible_rows();
        let origin=self.origin(rows);
        // One snapshot of the source for all slots, same sequence as `entry_at`; slot k maps to position k-1+origin.
        let first=origin.saturating_sub(1);
        let today=today_cached();
        // A scroll wheel repeats a full projection for the same window on every glide tick. This key
        // records everything the last full pass rendered; while the glide is running and nothing else
        // moved, only the row positions need repainting. Outside a glide the pass stays complete, so a
        // fresh document never inherits rows it did not write.
        let animating=self.glide.is_animating(Instant::now());
        let total=self.window.0-SIDEBAR_WIDTH;
        let key=RowsKey{
            epoch:state_epoch(),origin,rows,first,day:today,window_w:self.window.0,
            name_w:self.widths.name_width(total,kind_shown),mod_w:self.widths.modified,
            size_w:self.widths.size,kind_w:self.widths.kind,kind_shown,
            selected:self.theme.selected(),clear:self.theme.clear(),
            alternate:self.theme.row_alternate(),selection_fill:self.theme.selection_fill(),
        };
        if animating && ROWS_KEY.with(std::cell::Cell::get)==Some(key) {
            return self.project_rows_positions(doc);
        }
        let mut window: [Option<&Entry>; ROW_SLOTS] = [None; ROW_SLOTS];
        match self.search_rows.as_ref() {
            Some(results)=>{
                for (slot, entry) in results.iter().skip(first).take(ROW_SLOTS).enumerate() { window[slot]=Some(entry); }
            },
            // No active query (the common case): copy the window straight out of the sorted listing
            // instead of filtering from row 0 to the scroll origin on every full projection.
            None if tab.search.is_empty()=>{
                for (slot, entry) in tab.entries.get(first..).unwrap_or_default().iter().take(ROW_SLOTS).enumerate() { window[slot]=Some(entry); }
            },
            None=>{
                for (slot, entry) in tab.visible().skip(first).take(ROW_SLOTS).enumerate() { window[slot]=Some(entry); }
            },
        }
        // A full pass replaces the thumbnail requests, but only when the window it shows changed:
        // pruning and re-requesting the same rows cloned every queued key and re-hashed the pending
        // set on each wake. Search results are re-ordered when the walk finishes, so they always restart.
        // A job for a row that is still projected in view is kept as-is; only rows that left the view
        // lose their request (and are never decoded).
        let mut visible=[false;ROW_SLOTS];
        for (slot,shown) in visible.iter_mut().enumerate() {
            let y=top+(slot as f32-1.0+glide)*ROW_HEIGHT;
            *shown=y>=top-0.5&&y+ROW_HEIGHT<=bottom+0.5;
        }
        if let Some(queue)=self.thumb_queue.as_ref()
            && (self.search_rows.is_some() || thumb_pass_changed(tab,first,rows)) {
            self.thumbs.borrow_mut().start_pass_keeping(queue,|key|{
                // Window position `pos` holds the entry of absolute row `first + pos`, shown by slot
                // `first + pos - origin + 1`; a listing at its first row shifts by one because `first`
                // saturates at zero.
                window.iter().enumerate().any(|(pos,entry)|{
                    let slot=(first+pos) as isize-origin as isize+1;
                    let shown=usize::try_from(slot).ok().and_then(|slot|visible.get(slot).copied()).unwrap_or(false);
                    shown&&matches!(*entry,Some(entry)
                        if entry.path==key.path&&entry.size==key.size&&entry.modified==key.modified)
                })
            });
        }
        let ids=IdTables::get();
        let occupancy=ROW_LABELS.with(|cell|->Result<[bool;ROW_SLOTS],String>{
            let mut labels=cell.borrow_mut();
            let mut occupancy=[false;ROW_SLOTS];
            for slot in 0..ROW_SLOTS {
                let row=&ids.rows[slot];
                let index=flamewm_explorer::row_slot_index(origin,slot);
                let y=top+(slot as f32-1.0+glide)*ROW_HEIGHT;
                let entry=if slot<=rows+1 {usize::try_from(index).ok().and_then(|i|i.checked_sub(first)).and_then(|p|window.get(p).copied().flatten())} else {None};
                let in_view=visible[slot];
                occupancy[slot]=entry.is_some();
                if let Some(entry)=entry {
                    // Contents are set even off-view, so a row that glides into view later already shows its entry.
                    doc.visible(&row.row,in_view)?;
                    doc.position(&row.row,SIDEBAR_WIDTH,y)?;
                    doc.size(&row.row,self.window.0-SIDEBAR_WIDTH,ROW_HEIGHT)?;
                    let text=row_labels(&mut labels,slot,entry,today);

                    doc.text_ref(&row.size,&text.size)?;
                    doc.text_ref(&row.modified,&text.modified)?;
                    doc.text_ref(&row.kind,&text.kind)?;
                    doc.size(&row.modified,self.widths.modified,ROW_HEIGHT)?;
                    doc.size(&row.size,self.widths.size,ROW_HEIGHT)?;
                    doc.size(&row.kind,self.widths.kind,ROW_HEIGHT)?;
                    doc.visible(&row.kind,kind_shown)?;
                    // Dot files show the dimmed name twin (`name-dim-*`, CSS colour). Foreground overrides cannot reach a
                    // text child, and the icon raster is shared, so the icon stays full strength.
                    // Only the visible twin is rebound: the hidden one is refreshed the next time it shows.
                    let dim=entry.name.starts_with('.');
                    doc.visible(&row.name,!dim)?;
                    doc.visible(&row.name_dim,dim)?;
                    if dim {doc.text_ref(&row.name_dim,&entry.name)?;} else {doc.text_ref(&row.name,&entry.name)?;}
                    // Inline rename: the name cell shows the field with its caret; a selected stem is highlighted.
                    if matches!(self.edit,Some(EditMode::Rename)) && self.rename_target.as_ref()==Some(&entry.path) {
                        doc.visible(&row.name,true)?;
                        doc.visible(&row.name_dim,false)?;
                        doc.text_ref(&row.name,&self.field.display())?;
                        doc.background(&row.name,if self.field.has_selection() {self.theme.selection_fill()} else {self.theme.clear()})?;
                    } else {doc.background(&row.name,self.theme.clear())?;}
                    self.project_row_icon(doc,slot,entry,in_view,text.icon,text.thumb.as_ref())?;
                    doc.background(&row.row,if self.drop_folder.as_deref()==Some(entry.path.as_path()) || tab.selected_paths.contains(&entry.path) {
                        self.theme.selected()
                    }
                        else if index%2==0{self.theme.clear()}else{self.theme.row_alternate()})?;
                } else {doc.visible(&row.row,false)?;}
            }
            Ok(occupancy)
        })?;
        ROW_OCCUPANCY.with(|cell|*cell.borrow_mut()=occupancy);
        ROWS_KEY.with(|cell|cell.set(Some(key)));
        Ok(())
    }

    /// Positions only: the rows the last full pass filled, moved to the current glide offset. Used by
    /// glide frames and by a repeated full pass for the same window.
    fn project_rows_positions(&self, doc:&mut impl UiDocumentAccess)->Result<(),String> {
        let top=list_top(self.model.tabs.len()>1);
        let bottom=self.window.1-STATUS_STRIP_HEIGHT;
        let glide=self.glide.offset(Instant::now());
        let ids=IdTables::get();
        ROW_OCCUPANCY.with(|occupancy|->Result<(),String>{
            for (slot,occupied) in occupancy.borrow().iter().enumerate() {
                if !*occupied {continue;}
                let row=&ids.rows[slot];
                let y=top+(slot as f32-1.0+glide)*ROW_HEIGHT;
                let in_view=y>=top-0.5&&y+ROW_HEIGHT<=bottom+0.5;
                doc.visible(&row.row,in_view)?;
                doc.position(&row.row,SIDEBAR_WIDTH,y)?;
            }
            Ok(())
        })
    }

    pub(crate) fn project_row_icon(&self, doc:&mut impl UiDocumentAccess, slot:usize, entry:&Entry, in_view:bool, icon:&'static str, thumb:Option<&ThumbKey>)->Result<(),String> {
        // The key is built with the row labels, so a full pass no longer clones the path per image row.
        let thumbnail=thumb.and_then(|key|self.row_thumbnail(key,in_view));
        let ids=IdTables::get();
        let ids=&ids.rows[slot];
        // Only the thumbnail branch asks `row_icon_choice`; the type branch reuses the icon family the
        // row labels already resolved instead of lowercasing the extension a second time.
        let choice=if thumbnail.is_some() {row_icon_choice(entry,thumbnail.as_ref())} else {RowIcon::Type(icon)};
        match choice {
            RowIcon::Thumbnail(id)=>{
                let Some((_,pixels))=thumbnail else {return self.show_type_icon(doc,slot,icon)};
                // The thumbnail draws in the same square as the type glyph. The element is sized to
                // ROW_ICON_BOX explicitly, which overrides the stylesheet's larger `.row-thumb` rule:
                // the name cell never jumps when a thumbnail arrives, and the 24px rule can never
                // squeeze the 18px glyph out of its cell.
                let mut bound=self.thumb_bound.borrow_mut();
                if bound[slot]!=Some(id) {
                    // A document that rejects the raster must not blank the row (or abort the whole
                    // projection, which would leave every later slot with a stale icon): fall back to
                    // the embedded type icon for this slot instead.
                    let bound_ok=doc.image_rgba8_shared(&ids.ico_thumb,format!("explorer-thumb-{id}"),THUMB_SIZE,THUMB_SIZE,pixels).is_ok();
                    bound[slot]=bound_ok.then_some(id);
                    if !bound_ok {drop(bound);return self.show_type_icon(doc,slot,icon);}
                    doc.size(&ids.ico_thumb,ROW_ICON_BOX,ROW_ICON_BOX)?;
                    for icon_id in &ids.ico {doc.visible(icon_id,false)?;}
                    doc.visible(&ids.ico_thumb,true)?;
                    TYPE_ICON_SHOWN.with(|shown|shown.borrow_mut()[slot]=None);
                }
            }
            RowIcon::Type(icon)=>self.show_type_icon(doc,slot,icon)?,
        }
        Ok(())
    }

    /// Shows one embedded `row-*.svg` family icon and hides the thumbnail. The family name always
    /// comes from `view::file_icon`, whose values are the compiled `ROW_ICONS`. The glyph is sized
    /// explicitly so it stays visible even if the stylesheet's `.row-ico` box changes.
    pub(crate) fn show_type_icon(&self, doc:&mut impl UiDocumentAccess, slot:usize, icon:&'static str)->Result<(),String> {
        // A full projection re-renders every visible row; most rows keep the same family, so the
        // hide-all/show-one pass is skipped unless the slot's icon or its thumbnail binding changed.
        let unchanged=self.thumb_bound.borrow()[slot].is_none()
            && TYPE_ICON_SHOWN.with(|shown|shown.borrow()[slot])==Some(icon);
        if unchanged {return Ok(());}
        self.thumb_bound.borrow_mut()[slot]=None;
        let ids=IdTables::get();
        let ids=&ids.rows[slot];
        doc.visible(&ids.ico_thumb,false)?;
        for (kind,icon_id) in ROW_ICONS.iter().zip(&ids.ico) {
            let shown=*kind==icon;
            doc.visible(icon_id,shown)?;
            if shown {doc.size(icon_id,ROW_ICON_BOX,ROW_ICON_BOX)?;}
        }
        TYPE_ICON_SHOWN.with(|shown|shown.borrow_mut()[slot]=Some(icon));
        Ok(())
    }

    /// Cached thumbnail for an image row. A cache miss queues one decode and shows the type icon meanwhile.
    /// Only rows in view queue a decode: a row scrolled off-screen in the same pass is not decoded.
    pub(crate) fn row_thumbnail(&self, key:&ThumbKey, in_view:bool)->Option<(u64,Arc<Vec<u8>>)> {
        let mut cache=self.thumbs.borrow_mut();
        match cache.lookup(key) {
            Lookup::Ready(id,pixels)=>Some((id,pixels)),
            Lookup::Failed=>None,
            Lookup::Missing=>{
                if in_view && cache.begin(key)
                    && let Some(queue)=&self.thumb_queue {queue.push(key.clone());}
                None
            }
        }
    }

    /// Title, body lines and the four button slots (`None` hides a slot) for an open dialog.
    pub(crate) fn dialog_model(dialog:&Dialog)->(String,Vec<String>,[Option<&'static str>;4]) {
        match dialog {
            Dialog::Properties{props,size,..}=>(
                props.path.file_name().unwrap_or(props.path.as_os_str()).to_string_lossy().into_owned(),
                vec![
                    props.path.display().to_string(),
                    format!("Size: {}",size.map_or("calculating…".to_owned(),flamewm_api::format::human_size)),
                    format!("Permissions: {}",properties::mode_string(props.mode)),
                    format!("Owner: uid {} · group {}",props.uid,props.gid),
                ],
                [Some("Edit Permissions"),None,None,Some("Close")],
            ),
            Dialog::Conflicts{count,..}=>(
                "Items already exist".into(),
                vec![
                    format!("{count} item(s) already exist in this folder."),
                    "Skip leaves the existing items unchanged.".into(),
                    "Keep Both adds a numbered copy beside them.".into(),
                    "Replace removes the existing items after confirmation.".into(),
                ],
                [Some("Skip"),Some("Keep Both"),Some("Replace…"),Some("Cancel")],
            ),
            Dialog::Confirm{title,lines,..}=>(title.clone(),lines.clone(),[Some("OK"),None,None,Some("Cancel")]),
            Dialog::HostKey{url,prompt}=>match prompt {
                Some(key)=>(
                    "Trust this server?".into(),
                    vec![
                        format!("{} ({})",remote::display_text(url),key.key_type),
                        format!("Fingerprint {}",key.fingerprint_sha256),
                        "Compare it with the server's real fingerprint before trusting.".into(),
                        "Trusting saves the key to ~/.ssh/known_hosts.".into(),
                    ],
                    [Some("Trust Key"),None,None,Some("Cancel")],
                ),
                None=>(
                    "Checking host key".into(),
                    vec![format!("Fetching the host key for {}…",remote::display_text(url)),String::new(),String::new(),String::new()],
                    [None,None,None,Some("Cancel")],
                ),
            },
            Dialog::AuthFailed{url,password_possible}=>(
                "Sign-in failed".into(),
                vec![
                    format!("The server did not accept a key for {}.",remote::display_text(url)),
                    "Add a key to ~/.ssh or load an ssh agent, then retry.".into(),
                    if *password_possible {"The server wants a password, which FlameWM does not ask for.".into()} else {String::new()},
                    String::new(),
                ],
                [Some("Retry"),None,None,Some("Close")],
            ),
        }
    }

    pub(crate) fn project_dialog(&self, doc:&mut impl UiDocumentAccess)->Result<(),String> {
        let Some(dialog)=self.dialog.as_ref() else {
            doc.visible("dialog",false)?;
            return Ok(());
        };
        doc.visible("dialog",true)?;
        doc.background("dialog",self.theme.menu())?;
        // Keep the modal inside small windows: its CSS width (460px) and fixed top are clamped to the window.
        let dialog_w=460f32.min(self.window.0-16.0).max(1.0);
        doc.size("dialog",dialog_w,200.0)?;
        doc.position("dialog",((self.window.0-dialog_w)/2.0).max(0.0),150f32.min((self.window.1-200.0).max(8.0)))?;
        let (title,lines,buttons)=Self::dialog_model(dialog);
        let ids=IdTables::get();
        doc.text("dialog-title",title)?;
        for (i,line) in (0..4).zip(lines.into_iter().chain(std::iter::repeat(String::new()))) {
            doc.text(&ids.dialog_lines[i],line)?;
        }
        for (i,label) in buttons.iter().enumerate() {
            doc.visible(&ids.dialog_buttons[i],label.is_some())?;
            if let Some(label)=label {doc.text(&ids.dialog_buttons[i],(*label).to_owned())?;}
        }
        // The focused button carries the ring; Enter activates it (WAI-ARIA APG alertdialog).
        let focused=super::input::dialog_focus_slot(&buttons,self.dialog_focus).map(|i|ids.dialog_buttons[i].as_str());
        doc.set_focus_node(focused)?;
        Ok(())
    }

    pub(crate) fn project_chooser(&self, doc:&mut impl UiDocumentAccess)->Result<(),String> {
        let showing=self.chooser_loading || self.chooser.is_some();
        doc.visible("openwith-dialog",showing)?;
        if !showing{return Ok(());}
        // Clamp the chooser (440x466 in CSS) to the window so its action row stays reachable at 520x420 and 320x240.
        let chooser_w=440f32.min(self.window.0-16.0).max(1.0);
        let chooser_h=466f32.min(self.window.1-16.0).max(1.0);
        doc.size("openwith-dialog",chooser_w,chooser_h)?;
        // The app list takes what the fixed header (77), pages (35) and actions (85) leave, so the action row stays inside the window.
        doc.size("openwith-apps",chooser_w-2.0,(chooser_h-2.0-77.0-35.0-85.0).max(0.0))?;
        doc.position("openwith-dialog",((self.window.0-chooser_w)/2.0).max(0.0),((self.window.1-chooser_h)/2.0).max(0.0))?;
        doc.background("openwith-dialog",self.theme.menu())?;
        doc.background("openwith-toolbar",self.theme.card())?;
        if let Some(chooser)=self.chooser.as_ref() {
            let filename=chooser.path.file_name().unwrap_or_default().to_string_lossy();
            doc.text("openwith-title",format!("Open {filename} with"))?;
            doc.text("openwith-mime",format!("{} · {} compatible apps",chooser.mime,
                chooser.apps.iter().filter(|app|app.associated).count()))?;
            let choices=chooser.page_choices();
            let ids=IdTables::get();
            for (slot,slot_ids) in ids.chooser.iter().enumerate() {
                let id=&slot_ids.choice;
                if let Some((index,choice))=choices.get(slot) {
                    doc.visible(id,true)?;
                    let suffix=if choice.is_default {" · Default"} else if !choice.associated {" · Other app"} else {""};
                    doc.text(&slot_ids.label,format!("{}{suffix}",choice.entry.name()))?;
                    let icon_id=&slot_ids.icon;
                    match choice.icon.as_ref() {
                        Some(icon)=>{
                            // Rows bind their icon by application id, so a slot reused for another app rebinds on change.
                            // A rejected raster hides the slot instead of aborting the projection: a failed bind must
                            // never leave the dialog (or the rows below it) unprojected.
                            let source=format!("explorer-app-icon-{}",choice.entry.id().as_str());
                            let bound=doc.image_rgba8_shared(icon_id,source,icon.width,icon.height,Arc::clone(&icon.pixels)).is_ok();
                            doc.visible(icon_id,bound)?;
                        }
                        None=>doc.visible(icon_id,false)?,
                    }
                    doc.background(id,if chooser.selected==Some(*index){self.theme.selected()}else{self.theme.card()})?;
                }else{doc.visible(id,false)?;}
            }
            doc.text("openwith-page",format!("{} / {}",chooser.page+1,chooser.page_count()))?;
            doc.visible("openwith-prev",chooser.page_count()>1)?;
            doc.visible("openwith-next",chooser.page_count()>1)?;
            doc.text("openwith-all",if chooser.show_all {"Compatible apps".into()}else{"All apps".into()})?;
            doc.visible("openwith-all",chooser.apps.iter().any(|app|!app.associated))?;
            doc.visible("openwith-once",chooser.selected.is_some())?;
            doc.visible("openwith-always",chooser.selected.is_some())?;
        }else{
            doc.text("openwith-title","Finding installed applications…".into())?;
            doc.text("openwith-mime","Reading desktop MIME handlers".into())?;
            let ids=IdTables::get();
            for slot_ids in &ids.chooser {doc.visible(&slot_ids.choice,false)?;}
            for id in ["openwith-prev","openwith-next","openwith-all","openwith-once","openwith-always"] {doc.visible(id,false)?;}
            doc.text("openwith-page",String::new())?;
        }
        Ok(())
    }
}
