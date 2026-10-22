//! FlameWM Explorer — native X11 retained UI; scans and transfers have separate workers.
mod theme;
mod mime;
use mime::{OpenWithChooser, CHOICE_ROWS};
use std::cell::RefCell;
use std::fs;
use std::io::{Read,Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path,PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc::{self,Receiver,Sender};
use std::time::{Duration,Instant};

use flamewm_clipboard::{Clipboard,FileOp};
use flamewm_explorer::clipboard::{resolve_paste,settle_cut};
use flamewm_explorer::{prefs,AddressTarget,BatchReport,Entry,Explorer,Place,PlaceGroup,SidebarRow,SortBy,sidebar_rows,ROW_SLOTS,add_bookmark,display_location,home_dir,is_archive,list_directory,parse_address,places};
use flamewm_explorer::view::{self,Column,ColumnSort};
use flamewm_explorer::{archive,properties,refresh::DirWatch,search::{SearchEvent,SearchWorker},trash,places as bookmarks};
use std::sync::atomic::{AtomicBool,Ordering};
use std::sync::Arc;
use flamewm_explorer::jobs::{self,CancellationToken,Conflict,Undo};
use theme::ExplorerTheme;
use flamewm_control_core::{ControlRequest,ControlResponse};
use flamewm_control_dbus::{ControlClient,ControlSignal,ControlSignalClient};
use flamewm_dbus_reactor::BusKind;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_desktop_core::{file_actions::create_new_folder,trash::Trash};
use flamewm_ui_x11::{UiActionEvent,UiActionPhase,UiColor,UiControllerEvent,UiDocumentAccess,UiKeyEvent,UiKeyPhase,UiWakeDrain,UiWindowConfig,UiWindowRole,run_with_controller_events_role_and_two_readable_fds,decode_document};

const UI: &[u8]=include_bytes!(concat!(env!("OUT_DIR"),"/flamewm-explorer.rwr"));
const MENU_ITEMS: [&str;23]=["open","openwith","cut","copy","paste","rename","duplicate","newfolder","bookmark","trash","properties","terminal","compress","extract","newtab","opencontaining","restore","emptytrash","placerename","placeremove","unmount","eject","hidden"];
/// Sidebar rows (group headings plus places) that the template provides; extra rows are not drawn.
const SIDEBAR_SLOTS:usize=32;
// Canonical geometry: paint (project) and hit-testing (cell_rect, folder_at, select bands) all derive from these.
const TOOLBAR_HEIGHT:f32=45.0;
const TABBAR_HEIGHT:f32=36.0;
const HEADER_HEIGHT:f32=29.0;
const ROW_HEIGHT:f32=35.0;
/// Status strip (30px) plus the hairline above it; the file rows fill the space above.
const STATUS_STRIP_HEIGHT:f32=31.0;
/// Rows moved by one wheel notch.
const WHEEL_ROWS:isize=3;
const SIDEBAR_WIDTH:f32=201.0;
const DRAG_THRESHOLD_PX:f32=7.0;
// Maximum window size; the actual size is clamped to 90% of the root (see `window_size`).
const WINDOW_WIDTH:u32=1250;
const WINDOW_HEIGHT:u32=740;
/// Icon families compiled into the template; each slot toggles the one it needs (`pico-<slot>-<kind>`, `ico-<slot>-<kind>`).
const PLACE_ICONS:[&str;11]=["home","desktop","documents","downloads","music","pictures","videos","trash","folder","drive","usb"];
const ROW_ICONS:[&str;4]=["folder","pdf","source","file"];
/// Width of the status-strip usage track (`.usage-track` in explorer.css).
const USAGE_TRACK_WIDTH:f32=114.0;

/// Height kept free at the bottom of the screen for the taskbar panel.
const TASKBAR_RESERVE:u32=56;
/// Top edge of the window when there is room for it.
const WINDOW_TOP:u32=65;
/// WM frame chrome around the client: a 31px titlebar (flamewm-wm-x11 frame/layout `TITLEBAR_H`) plus a 1px border on each side.
/// The WM draws it outside the client rect, so the height budget must include it or the bottom of the client (the status strip) lands under the taskbar.
const FRAME_CHROME_HEIGHT:u32=31+2;
/// Client size and top edge. The whole framed window (client plus chrome) is clamped to 90% of the work area (root minus taskbar),
/// so the status strip stays above the taskbar at any screen size.
fn window_frame(root:(u32,u32))->(u32,u32,i32) {
    let work_w=root.0;
    let work_h=root.1.saturating_sub(TASKBAR_RESERVE);
    let w=WINDOW_WIDTH.min((u64::from(work_w)*9/10) as u32).max(1);
    let frame_h=(WINDOW_HEIGHT+FRAME_CHROME_HEIGHT).min((u64::from(work_h)*9/10) as u32);
    let h=frame_h.saturating_sub(FRAME_CHROME_HEIGHT).max(1);
    let y=WINDOW_TOP.min(work_h.saturating_sub(frame_h));
    (w,h,y as i32)
}
/// Top of the column header: below the toolbar and, with more than one tab, the tab strip.
fn header_top(tab_strip:bool)->f32 {TOOLBAR_HEIGHT+if tab_strip {TABBAR_HEIGHT} else {0.0}}
/// Top of the first row, directly below the column header.
fn list_top(tab_strip:bool)->f32 {header_top(tab_strip)+HEADER_HEIGHT}
/// Glide frame interval and per-frame retention for sub-row scroll animation (about 150 ms to settle).
const SCROLL_FRAME:Duration=Duration::from_millis(16);
const SCROLL_EASE:f32=0.6;
/// Typing pauses longer than this start a fresh type-ahead prefix.
const TYPEAHEAD_RESET:Duration=Duration::from_millis(800);

/// A pressed file row can become a drag only after measurable pointer motion.
/// Release outside a folder cancels without changing files or the clipboard.
#[derive(Debug)]
struct RowDrag {
    source:PathBuf,
    origin:(f32,f32),
    dragging:bool,
}
impl RowDrag {
    fn new(source:PathBuf,x:f32,y:f32)->Self { Self{source,origin:(x,y),dragging:false} }
    fn motion(&mut self,x:f32,y:f32)->bool {
        if (x-self.origin.0).hypot(y-self.origin.1)>=DRAG_THRESHOLD_PX {self.dragging=true;}
        self.dragging
    }
}

enum WorkerJob {
    Scan {generation:u64,path:PathBuf,hidden:bool,sort:SortBy},
    Transfer {sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,conflict:Conflict,cancel:CancellationToken},
    Drop {sources:Vec<PathBuf>,directory:PathBuf,force_copy:bool,cancel:CancellationToken},
    Trash {paths:Vec<PathBuf>},
    Delete {paths:Vec<PathBuf>},
    EmptyTrash {base:PathBuf},
    Restore {base:PathBuf,stored:PathBuf},
    Create {directory:PathBuf},
    Rename {from:PathBuf,to:PathBuf},
    Undo,
    Redo,
}
enum ChooserJob {
    Discover { request: u64, path: PathBuf },
    SetDefault { mime: String, desktop_id: String },
}
fn chooser_worker(rx: Receiver<ChooserJob>, result: Sender<WorkerResult>, mut wake: UnixStream) {
    for job in rx {
        match job {
            ChooserJob::Discover { request, path } => {
                let chooser = mime::discover(path);
                send_wake(&mut wake, &result, WorkerResult::ChooserReady { request, chooser });
            }
            ChooserJob::SetDefault { mime, desktop_id } => {
                let outcome = mime::set_default(&mime, &desktop_id);
                send_wake(&mut wake, &result, WorkerResult::DefaultSaved(outcome));
            }
        }
    }
}
enum WorkerResult {
    Scanned {generation:u64,path:PathBuf,entries:std::io::Result<Vec<Entry>>},
    Progress(u64),
    Total(u64),
    /// Recursive size of a folder shown in Properties; `path` identifies the dialog it belongs to.
    SizeReady {path:PathBuf,bytes:std::io::Result<u64>},
    /// One frame of the list scroll glide.
    ScrollTick,
    /// Search worker has a new poll result; the app drains it on the UI thread.
    SearchTick,
    /// Directory watcher saw a change in the listed folder.
    DirChanged,
    ChooserReady { request: u64, chooser: Result<OpenWithChooser, String> },
    DefaultSaved(Result<(), String>),
    BatchCompleted {report:BatchReport,action:&'static str,cut_sources:Option<Vec<PathBuf>>},
    Completed(Result<String,String>),
    /// Unmounted removable filesystems from lsblk.
    Devices(Vec<bookmarks::BlockDevice>),
    /// Outcome of a udisks2 mount/unmount/eject, already worded for the status strip.
    DeviceDone(Result<String,String>),
    /// Paste decision from the system clipboard read; `None` means nothing to paste.
    PasteResolved(Option<(Vec<PathBuf>,bool)>),
}
fn send_wake(wake:&mut UnixStream, result:&Sender<WorkerResult>, message:WorkerResult) {
    if result.send(message).is_ok() {let _=wake.write_all(&[1]);}
}
fn scan_worker(rx:Receiver<WorkerJob>, result:Sender<WorkerResult>, mut wake:UnixStream) {
    while let Ok(mut job)=rx.recv() {
        // Obsolete queued navigation requests must never block the latest scan.
        while let Ok(newer)=rx.try_recv() { job=newer; }
        if let WorkerJob::Scan{generation,path,hidden,sort}=job {
            send_wake(&mut wake,&result,WorkerResult::Scanned{generation,entries:list_directory(&path,hidden,sort),path});
        }
    }
}
/// Moves one path to the user's Trash and returns the restorable journal entry.
fn trash_one(path:&Path)->std::io::Result<Undo> {
    let home=home_dir();
    let data=std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".local/share"));
    let uid=fs::read_to_string("/proc/self/status").ok()
        .and_then(|s|flamewm_desktop_core::trash::current_uid_from_proc(&s))
        .ok_or_else(||std::io::Error::new(std::io::ErrorKind::PermissionDenied,"cannot determine current user"))?;
    Trash::move_to_trash(path,&home,&data,uid)
        .map(|item|Undo::Trashed{original:item.original,stored:item.stored,info:item.info})
        .map_err(|e|std::io::Error::other(e.to_string()))
}
/// Undo (`undoing`) or redo the newest entry of the bounded history and report it in status text.
fn history_step(undoing:bool,trash:&mut dyn FnMut(&Path)->std::io::Result<Undo>)->Result<String,String> {
    let mut journal=jobs::journal().lock().unwrap_or_else(|poison|poison.into_inner());
    if undoing {
        let Some(entry)=journal.take_last() else {return Err("Nothing to undo".to_owned());};
        match jobs::undo_with(&entry,trash) {
            Ok(())=>{let text=format!("Undid: {}",jobs::describe(&entry));journal.push_redo(entry);Ok(text)},
            Err(error)=>{journal.restore(entry);Err(format!("Undo failed: {error}"))},
        }
    } else {
        let Some(entry)=journal.take_redo() else {return Err("Nothing to redo".to_owned());};
        match jobs::redo_with(&entry,trash) {
            Ok(next)=>{let text=format!("Redid: {}",jobs::describe(&next));journal.push_undo(next);Ok(text)},
            Err(error)=>{journal.push_redo(entry);Err(format!("Redo failed: {error}"))},
        }
    }
}
fn worker(rx:Receiver<WorkerJob>, result:Sender<WorkerResult>, mut wake:UnixStream) {
    for job in rx {
        match job {
            WorkerJob::Scan{..}=>{},
            WorkerJob::Transfer{sources,directory,move_it,conflict,cancel}=>{
                let total=jobs::count_sources(&sources,&cancel).bytes;
                send_wake(&mut wake,&result,WorkerResult::Total(total));
                let mut accumulated=0_u64;
                let mut last_progress=Instant::now();
                let mut progress=|size:u64|{
                    accumulated=accumulated.saturating_add(size);
                    if last_progress.elapsed()>=Duration::from_millis(100) {
                        send_wake(&mut wake,&result,WorkerResult::Progress(accumulated));
                        last_progress=Instant::now();
                    }
                };
                let mut report=jobs::transfer_batch_with(&sources,&directory,move_it,conflict,&cancel,&mut progress);
                jobs::record(report.undo.drain(..));
                send_wake(&mut wake,&result,WorkerResult::BatchCompleted {
                    report,action:if move_it {"Moved"}else{"Copied"},
                    cut_sources:move_it.then_some(sources),
                });
            }
            WorkerJob::Drop{sources,directory,force_copy,cancel}=>{
                send_wake(&mut wake,&result,WorkerResult::Total(jobs::count_sources(&sources,&cancel).bytes));
                let mut accumulated=0_u64;
                let mut last_progress=Instant::now();
                let mut progress=|size:u64|{
                    accumulated=accumulated.saturating_add(size);
                    if last_progress.elapsed()>=Duration::from_millis(100) {
                        send_wake(&mut wake,&result,WorkerResult::Progress(accumulated));
                        last_progress=Instant::now();
                    }
                };
                let mut report=jobs::transfer_drop_batch_with(&sources,&directory,force_copy,&cancel,&mut progress);
                jobs::record(report.undo.drain(..));
                send_wake(&mut wake,&result,WorkerResult::BatchCompleted {
                    report,action:"Transferred",cut_sources:None,
                });
            }
            WorkerJob::Trash{paths}=>{
                let mut report=BatchReport::default();
                for path in &paths {
                    match trash_one(path) {
                        Ok(undo)=>{report.completed+=1;report.undo.push(undo);},
                        Err(error)=>report.record(path,Err(error)),
                    }
                }
                jobs::record(report.undo.drain(..));
                send_wake(&mut wake,&result,WorkerResult::BatchCompleted{report,action:"Trashed",cut_sources:None});
            }
            WorkerJob::Delete{paths}=>{
                // Permanent delete is only reached after the user confirms; nothing is journaled.
                let mut report=BatchReport::default();
                for path in &paths {
                    let meta=fs::symlink_metadata(path);
                    let removed=meta.and_then(|m|if m.is_dir() {fs::remove_dir_all(path)} else {fs::remove_file(path)});
                    report.record(path,removed);
                }
                send_wake(&mut wake,&result,WorkerResult::BatchCompleted{report,action:"Deleted",cut_sources:None});
            }
            WorkerJob::EmptyTrash{base}=>{
                let mut report=BatchReport::default();
                for entry in trash::list(&base).unwrap_or_default() {
                    let removed=[&entry.stored,&entry.info].iter().try_for_each(|p|fs::remove_file(p).or_else(|_|fs::remove_dir_all(p)));
                    report.record(&entry.stored,removed);
                }
                send_wake(&mut wake,&result,WorkerResult::BatchCompleted{report,action:"Emptied Trash",cut_sources:None});
            }
            WorkerJob::Restore{base,stored}=>{
                let found=trash::list(&base).unwrap_or_default().into_iter().find(|e|e.stored==stored);
                let done=match found {
                    Some(entry)=>trash::restore(&entry).map(|dest|format!("Restored to {}",dest.display())).map_err(|e|e.to_string()),
                    None=>Err("Item is no longer in the Trash".to_owned()),
                };
                send_wake(&mut wake,&result,WorkerResult::Completed(done));
            }
            WorkerJob::Create{directory}=>{
                let done=create_new_folder(&directory);
                if let Ok(path)=&done {jobs::record([Undo::Created{path:path.clone()}]);}
                send_wake(&mut wake,&result,WorkerResult::Completed(done.map(|path|format!("Created {}",path.display())).map_err(|e|e.to_string())));
            }
            WorkerJob::Rename{from,to}=>{
                let done=flamewm_desktop_core::file_actions::rename_entry_no_replace(&from,&to);
                if done.is_ok() {jobs::record([Undo::Moved{from:from.clone(),to:to.clone()}]);}
                send_wake(&mut wake,&result,WorkerResult::Completed(done.map(|_|"Renamed".to_owned()).map_err(|e|e.to_string())));
            }
            WorkerJob::Undo=>{
                let done=history_step(true,&mut trash_one);
                send_wake(&mut wake,&result,WorkerResult::Completed(done));
            }
            WorkerJob::Redo=>{
                let done=history_step(false,&mut trash_one);
                send_wake(&mut wake,&result,WorkerResult::Completed(done));
            }
        }
    }
}
/// Work that waits for a dialog answer before it touches files.
enum Pending {
    Delete(Vec<PathBuf>),
    EmptyTrash(PathBuf),
    Transfer {sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,conflict:Conflict},
}
/// One modal at a time. Buttons map to fixed slots: primary, secondary, tertiary, cancel.
enum Dialog {
    Properties {props:properties::Properties, size:Option<u64>, cancel:Arc<AtomicBool>},
    Conflicts {sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,count:usize},
    Confirm {text:String, pending:Pending},
}
#[derive(Clone,Copy)]
enum EditMode { Path, Search, Rename, PlaceName, Mode }
struct ExplorerApp {
    model:Explorer, tx:Sender<WorkerJob>, scan_tx:Sender<WorkerJob>,
    chooser_tx:Sender<ChooserJob>, chooser:Option<OpenWithChooser>,
    chooser_loading:bool, chooser_request:u64, places:Vec<Place>,
    context:Option<(f32,f32)>, edit:Option<EditMode>, buffer:String, theme:ExplorerTheme, window:(f32,f32),
    last_click:Option<(PathBuf,Instant)>, progress:Option<u64>,
    context_on_entry:bool, rubber:Option<((f32,f32),(f32,f32))>, activity:Option<String>, busy:bool,
    row_drag:Option<RowDrag>, drop_folder:Option<PathBuf>, cancel:CancellationToken,
    menu_overflow:bool, scroll_offset:f32, scroll_origin:usize, scroll_ticking:bool,
    column_sort:ColumnSort, typed:String, typed_at:Option<Instant>,
    dialog:Option<Dialog>, ark:bool, search:Option<SearchWorker>, search_id:u64, search_text:String,
    search_rows:Option<Vec<Entry>>, watch:Option<DirWatch>, watched:Option<PathBuf>,
    context_place:Option<usize>, progress_total:u64, trash_base:PathBuf,
    /// Removable filesystems that are present but unmounted; merged into the sidebar's Devices group.
    unmounted:Vec<bookmarks::BlockDevice>,
    /// Wake channel for background threads (watcher, search, size). Set once in `run`.
    events:Option<(Sender<WorkerResult>,UnixStream)>,
    /// Cut sources the explorer already moved; the system clipboard still lists them, so they are not pasted twice.
    system_spent:Option<Vec<PathBuf>>,
}
impl ExplorerApp {
    fn new(initial:PathBuf,tx:Sender<WorkerJob>,scan_tx:Sender<WorkerJob>,chooser_tx:Sender<ChooserJob>) -> Self {
        let home=home_dir();
        let trash_base=std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".local/share")).join("Trash");
        let mut app=Self {model:Explorer::new(initial),tx,scan_tx,chooser_tx,chooser:None,chooser_loading:false,chooser_request:0,places:Vec::new(),context:None,edit:None,buffer:String::new(),theme:ExplorerTheme::default(),window:(WINDOW_WIDTH as f32,WINDOW_HEIGHT as f32),last_click:None,progress:None,context_on_entry:false,rubber:None,activity:None,busy:false,row_drag:None,drop_folder:None,cancel:CancellationToken::new(),menu_overflow:false,scroll_offset:0.0,scroll_origin:0,scroll_ticking:false,column_sort:ColumnSort::default(),typed:String::new(),typed_at:None,
            dialog:None,ark:archive::find_ark().is_some(),search:None,search_id:0,search_text:String::new(),search_rows:None,watch:None,watched:None,context_place:None,progress_total:0,trash_base,unmounted:Vec::new(),events:None,system_spent:None};
        app.reload_places();
        app
    }
    /// Built-in and bookmark places, plus the Trash folder just before mounted devices.
    fn reload_places(&mut self) {
        let home=home_dir();
        let mut list=places(&home,&self.unmounted);
        // Trash sits at the end of the Favorites group, as in the mock.
        let trash=Place{name:"Trash".into(),path:self.trash_base.join("files"),removable:false,group:PlaceGroup::Favorites,icon:"trash",device:None};
        let at=list.iter().rposition(|p|p.group==PlaceGroup::Favorites).map_or(0,|i|i+1);
        list.insert(at,trash);
        self.places=list;
    }
    /// Runs `job` on its own thread and wakes the UI with its result. Used for lsblk and udisks2, which can block.
    fn spawn_background(&self,job:impl FnOnce()->WorkerResult+Send+'static) {
        let Some((tx,wake))=self.events.as_ref() else {return};
        let (tx,Ok(mut wake))=(tx.clone(),wake.try_clone()) else {return};
        std::thread::spawn(move||{if tx.send(job()).is_ok() {let _=wake.write_all(&[1]);}});
    }
    fn refresh_devices(&self) {
        self.spawn_background(||WorkerResult::Devices(bookmarks::unmounted_devices()));
    }
    fn clear_search(&mut self) {
        self.search_rows=None;
        if let Some(search)=self.search.as_mut() {search.cancel();}
    }
    fn start_search(&mut self,query:String) {
        if query.trim().is_empty() {self.clear_search();self.model.status="Search cleared".into();return;}
        let root=self.model.tab().path().to_path_buf();
        let hidden=self.model.hidden;
        let query_text=query.clone();
        let started=self.search.as_mut().map(|search|search.start(root,query,hidden));
        match started {
            Some(Ok(id))=>{self.search_id=id;self.search_text=query_text;self.search_rows=Some(Vec::new());self.model.status="Searching…".into();},
            Some(Err(error))=>self.model.status=format!("Search unavailable: {error}"),
            None=>self.model.status="Search worker unavailable".into(),
        }
    }
    fn take_search_results(&mut self) {
        let Some(search)=self.search.as_mut() else {return};
        let events=search.poll();
        for event in events {
            match event {
                SearchEvent::Batch{hits,..}=>{
                    let rows=self.search_rows.get_or_insert_with(Vec::new);
                    rows.extend(hits.into_iter().map(|hit|Entry{path:hit.path,name:hit.name,directory:hit.directory,symlink:false,size:hit.size,modified:hit.modified}));
                    self.model.status=format!("Searching… {} found",rows.len());
                },
                SearchEvent::Done{truncated,cancelled,..}=>{
                    if cancelled {continue;}
                    let count=self.search_rows.as_ref().map_or(0,Vec::len);
                    self.model.status=format!("{count} result(s){}",if truncated {" (truncated)"} else {""});
                },
            }
        }
    }
    /// Entries shown in the file pane: search results replace the folder listing while active.
    /// First window row: the search-result window when searching, else the folder window.
    fn origin(&self,rows:usize)->usize {
        match self.search_rows.as_ref() {
            Some(results)=>self.model.tab().scroll.min(results.len().saturating_sub(rows)),
            None=>self.model.tab().window_start(rows),
        }
    }
    fn entry_at(&self,index:isize)->Option<&Entry> {
        let index=usize::try_from(index).ok()?;
        match self.search_rows.as_ref() {
            Some(results)=>results.get(index),
            None=>self.model.tab().visible_nth(index),
        }
    }
    /// Tracks the window origin after every input or wake. A small step glides from the old position;
    /// a folder change or a jump larger than the window snaps.
    /// Settled client size from the WM. Columns, rows and the body follow `window` in `project`; the scroll
    /// origin is re-clamped here so the visible window never starts past the last row.
    fn resize(&mut self,width:u32,height:u32) {
        self.window=(width.max(1) as f32,height.max(1) as f32);
        self.sync_scroll();
    }
    fn sync_scroll(&mut self) {
        let origin=self.origin(self.visible_rows());
        if origin!=self.scroll_origin {
            let step=origin as f32-self.scroll_origin as f32;
            self.scroll_offset=retarget_offset(self.scroll_offset,step,self.visible_rows());
            self.scroll_origin=origin;
        }
        if self.scroll_offset!=0.0 {self.schedule_scroll_tick();}
    }
    fn schedule_scroll_tick(&mut self) {
        if self.scroll_ticking {return;}
        self.scroll_ticking=true;
        self.spawn_background(||{std::thread::sleep(SCROLL_FRAME);WorkerResult::ScrollTick});
    }
    fn step_scroll(&mut self) {
        self.scroll_ticking=false;
        self.scroll_offset=ease_offset(self.scroll_offset);
        if self.scroll_offset!=0.0 {self.schedule_scroll_tick();}
    }
    /// Show or hide dot files, persisting the choice for the next Explorer window.
    fn toggle_hidden(&mut self) {
        self.model.hidden=!self.model.hidden;
        if let Err(error)=prefs::save(&prefs::config_dir(),&prefs::Prefs{show_hidden:self.model.hidden}) {
            self.model.status=format!("Could not save Explorer settings: {error}");
        }
        self.scan();
    }
    fn visible_entries(&self)->Vec<&Entry> {
        let rows=self.visible_rows();
        if let Some(results)=self.search_rows.as_ref() {
            let start=self.model.tab().scroll.min(results.len().saturating_sub(rows));
            return results.iter().skip(start).take(rows).collect();
        }
        self.model.tab().visible_window(rows)
    }
    /// Rows that fit between the column header and the status strip. Paint, hit-testing and keys all use this.
    fn visible_rows(&self)->usize {
        let bottom=self.window.1 as f32-STATUS_STRIP_HEIGHT;
        let fit=((bottom-list_top(self.model.tabs.len()>1))/ROW_HEIGHT).floor();
        fit.clamp(1.0,ROW_SLOTS as f32) as usize
    }
    fn sort_by(&mut self,column:Column) {
        self.column_sort=self.column_sort.click(column);
        view::sort_entries(&mut self.model.tab_mut().entries,self.column_sort);
    }
    /// Type-ahead moves the selection to the first visible name starting with the typed prefix.
    fn typeahead(&mut self,text:&str) {
        let now=Instant::now();
        if self.typed_at.is_none_or(|at|now.duration_since(at)>TYPEAHEAD_RESET) {self.typed.clear();}
        self.typed.push_str(text);
        self.typed_at=Some(now);
        let tab=self.model.tab();
        let names=(0..tab.filtered_count()).filter_map(|i|tab.visible_nth(i).map(|e|e.name.clone())).collect::<Vec<_>>();
        let refs=names.iter().map(String::as_str).collect::<Vec<_>>();
        let from=self.selected().as_ref().and_then(|p|tab.visible_index_of(p)).unwrap_or(0);
        if let Some(index)=view::typeahead_match(&refs,&self.typed,from) {
            let path=tab.visible_nth(index).expect("index from typeahead").path.clone();
            let rows=self.visible_rows();
            let tab=self.model.tab_mut();
            tab.select_one(path);
            tab.reveal(index,rows);
        }
    }
    fn scan(&mut self) {
        self.row_drag=None;
        self.drop_folder=None;
        self.activity=None;
        self.request_listing();
    }
    /// Lists the current folder without clearing transient feedback (used by live refresh).
    fn request_listing(&mut self) {
        let generation=self.model.next_generation();
        let path=self.model.tab().path().to_path_buf();
        if self.watched.as_ref()!=Some(&path) {
            self.watched=Some(path.clone());
            self.watch=self.events.as_ref().and_then(|(tx,wake)|{
                let (tx,wake)=(tx.clone(),wake.try_clone().ok()?);
                DirWatch::spawn(&path,move||{if tx.send(WorkerResult::DirChanged).is_ok() {let _=(&wake).write_all(&[1]);}}).ok()
            });
        }
        self.model.status="Loading directory…".into();
        if self.scan_tx.send(WorkerJob::Scan{generation,path,hidden:self.model.hidden,sort:self.model.sort}).is_err() {
            self.model.status="File worker unavailable".into();
        }
    }
    fn navigate(&mut self,path:PathBuf) {
        if path.is_dir() {self.clear_search();self.model.tab_mut().visit(path);self.scan();} else {self.model.status="Directory does not exist".into();}
        self.context=None;
    }
    /// Entry behind DOM row `explorer.row.<slot>`. Paint places slot k at window row k-1 (the
    /// glide keeps one buffer slot above the window), so hit-testing must use the same mapping.
    fn row_entry(&self,slot:usize) -> Option<&Entry> {
        self.entry_at(flamewm_explorer::row_slot_index(self.origin(self.visible_rows()),slot))
    }
    fn chosen(&self,slot:usize) -> Option<&Entry> {
        self.visible_entries().get(slot).copied()
    }
    fn selected(&self) -> Option<PathBuf> {self.model.tab().selected.clone()}
    /// Drop coordinates belong to the Explorer surface. Only visible folder rows
    /// inside the file pane are eligible; no guessed external-window targets.
    fn folder_at(&self,x:f32,y:f32)->Option<PathBuf> {
        if !x.is_finite() || !y.is_finite() || !(SIDEBAR_WIDTH..self.window.0).contains(&x) {return None;}
        let top=list_top(self.model.tabs.len()>1);
        if y<top || y>=top+ROW_HEIGHT*self.visible_rows() as f32 {return None;}
        let slot=((y-top)/ROW_HEIGHT) as usize;
        self.chosen(slot).filter(|entry|entry.directory).map(|entry|entry.path.clone())
    }
    fn finish_row_drag(&mut self,e:&UiActionEvent)->bool {
        let Some(drag)=self.row_drag.take() else {return false;};
        self.drop_folder=None;
        if !drag.dragging {return false;}
        self.last_click=None;
        self.context=None;
        let Some(destination)=self.folder_at(e.x,e.y) else {
            self.model.status="Drop cancelled (choose a folder)".into();
            return true;
        };
        let sources=if self.model.tab().selected_paths.contains(&drag.source) {
            self.selected_files()
        } else {
            vec![drag.source]
        };
        if flamewm_explorer::drop_is_noop(&sources,&destination) {
            self.model.status="Drop cancelled (nothing to move)".into();
            return true;
        }
        // Mixed selections: items already in the destination stay where they are.
        let sources:Vec<PathBuf>=sources.into_iter().filter(|source|source.parent()!=Some(destination.as_path())).collect();
        // Control modifies the operation at release time, exactly like file managers.
        // Worker validates real/canonical paths before touching files.
        if self.busy {
            self.model.status="File operation already in progress".into();
        } else if self.tx.send(WorkerJob::Drop {
            sources,directory:destination,force_copy:e.ctrl_down(),cancel:self.cancel.clone(),
        }).is_ok() {
            self.busy=true;
            self.progress=Some(0);
            self.activity=None;
            self.model.status="Transferring dropped items…".into();
        } else {
            self.model.status="Transfer worker unavailable".into();
        }
        true
    }
    fn open(&mut self, path:&Path) {
        if path.is_dir() {self.navigate(path.to_path_buf());return;}
        // A .desktop file is not executed as a shell script. XDG application handlers own launch policy.
        if is_archive(path) {
            match Command::new("ark").arg(path).spawn() {
                Ok(_)=>return,
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
                Err(e)=>{self.model.status=format!("Ark failed: {e}");return;},
            }
        }
        if let Err(e)=Command::new("xdg-open").arg(path).spawn() {self.model.status=format!("Open failed: {e}");}
    }
    fn start_transfer(&mut self,sources:Vec<PathBuf>,move_it:bool) {
        let directory=self.model.tab().path().to_path_buf();
        self.start_transfer_to(sources,directory,move_it);
    }
    fn start_transfer_to(&mut self,sources:Vec<PathBuf>,directory:PathBuf,move_it:bool) {
        if self.busy {self.model.status="File operation already in progress".into();return;}
        if sources.is_empty() {self.model.status="Select files first".into();return;}
        // Existing names need one answer for the whole batch; the default keeps both.
        let count=sources.iter().filter(|s|s.file_name().is_some_and(|n|fs::symlink_metadata(directory.join(n)).is_ok())).count();
        if count>0 {self.dialog=Some(Dialog::Conflicts{sources,directory,move_it,count});return;}
        self.send_transfer(sources,directory,move_it,Conflict::KeepBoth);
    }
    fn send_transfer(&mut self,sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,conflict:Conflict) {
        self.progress=Some(0);
        self.activity=None;
        self.model.status=format!("{} {} item(s)…",if move_it{"Moving"}else{"Copying"},sources.len());
        if self.tx.send(WorkerJob::Transfer{sources,directory,move_it,conflict,cancel:self.cancel.clone()}).is_err() {
            self.progress=None;
            self.model.status="Transfer worker unavailable".into();
        } else {self.busy=true;}
    }
    fn selected_files(&self)->Vec<PathBuf> {self.model.tab().ordered_selection()}
    fn request_open_with(&mut self, path:PathBuf) {
        self.context=None;
        self.context_on_entry=false;
        self.chooser_request=self.chooser_request.wrapping_add(1);
        self.chooser_loading=true;
        self.chooser=None;
        let request=self.chooser_request;
        if self.chooser_tx.send(ChooserJob::Discover{request,path}).is_err() {
            self.chooser_loading=false;
            self.model.status="Open With service unavailable".into();
        }
    }
    fn dismiss_chooser(&mut self) {
        self.chooser_request=self.chooser_request.wrapping_add(1);
        self.chooser_loading=false;
        self.chooser=None;
    }
    fn launch_chooser_choice(&mut self, always:bool) {
        let Some(chooser)=self.chooser.as_ref() else{return};
        let Some(index)=chooser.selected else{return};
        let Some(app)=chooser.apps.get(index) else{return};
        let (entry,path,mime,desktop_id)=(app.entry.clone(),chooser.path.clone(),
            chooser.mime.clone(),app.entry.id().as_str().to_owned());
        match mime::launch(&entry,&path) {
            Ok(())=>{
                self.model.status=format!("Opened with {}",entry.name());
                self.dismiss_chooser();
                if always && self.chooser_tx.send(ChooserJob::SetDefault{mime,desktop_id}).is_err(){
                    self.model.status="Opened file; MIME default could not be saved".into();
                }
            }
            Err(error)=>self.model.status=format!("Cannot launch {}: {error}",entry.name()),
        }
    }
    fn chooser_action(&mut self, action:&str) {
        match action {
            "cancel"=>self.dismiss_chooser(),
            "open"=>self.launch_chooser_choice(false),
            "always"=>self.launch_chooser_choice(true),
            "all"=>if let Some(chooser)=self.chooser.as_mut(){chooser.show_every_app();},
            "prev"=>if let Some(chooser)=self.chooser.as_mut(){chooser.page=chooser.page.saturating_sub(1);},
            "next"=>if let Some(chooser)=self.chooser.as_mut(){chooser.page=(chooser.page+1).min(chooser.page_count()-1);},
            _=>if let Some(slot)=action.strip_prefix("choice.").and_then(|id|id.parse::<usize>().ok()) {
                if let Some(chooser)=self.chooser.as_mut(){chooser.select_visible(slot);}
            }
        }
    }
    fn spawn_terminal(&mut self) {
        let directory=self.model.tab().path().to_path_buf();
        for program in ["x-terminal-emulator","konsole","xfce4-terminal","xterm"] {
            match Command::new(program).current_dir(&directory).spawn() {
                Ok(_)=>return,
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>continue,
                Err(e)=>{self.model.status=format!("Terminal failed: {e}");return;},
            }
        }
        self.model.status="No terminal emulator installed".into();
    }
    fn spawn_ark(&mut self, args:&[&str], path:&Path) {
        match Command::new("ark").args(args).arg(path).spawn() {
            Ok(_)=>self.model.status="Opened Ark".into(),
            Err(e)=>self.model.status=format!("Ark unavailable: {e}"),
        }
    }
    fn compress_selected(&mut self) {
        let paths=self.selected_files();
        if paths.is_empty() {return;}
        match Command::new("ark").arg("--add").args(&paths).spawn() {
            Ok(_)=>self.model.status="Opened Ark compression dialog".into(),
            Err(e)=>self.model.status=format!("Ark unavailable: {e}"),
        }
    }
    /// Ctrl+Z (`undoing`) and Ctrl+Shift+Z / Ctrl+Y redo, run on the file worker so the UI never blocks.
    fn history_last(&mut self,undoing:bool) {
        if self.busy {self.model.status="File operation already in progress".into();return;}
        let job=if undoing {WorkerJob::Undo} else {WorkerJob::Redo};
        if self.tx.send(job).is_ok() {self.busy=true;self.model.status=if undoing {"Undoing…"} else {"Redoing…"}.into();}
        else {self.model.status="File worker unavailable".into();}
    }
    /// Publishes a file list to the system CLIPBOARD on a detached thread, so the UI never waits on it.
    fn publish_system_clipboard(&mut self,paths:Vec<PathBuf>,cut:bool) {
        self.system_spent=None;
        std::thread::spawn(move||{
            let op=if cut {FileOp::Cut} else {FileOp::Copy};
            match Clipboard::connect() {
                Ok(clipboard)=>{let _=clipboard.set_files(&paths,op);},
                Err(e)=>eprintln!("flamewm-explorer: system clipboard unavailable: {e}"),
            }
        });
    }
    /// Reads the system clipboard on a worker thread; the result comes back as `PasteResolved`.
    fn request_paste(&mut self) {
        let in_app=self.model.clipboard.clone();
        let spent=self.system_spent.clone();
        let Some((tx,mut wake))=self.events.as_ref().and_then(|(tx,wake)|Some((tx.clone(),wake.try_clone().ok()?))) else {
            self.apply_paste(resolve_paste(None,in_app.as_ref(),spent.as_deref()));
            return;
        };
        self.model.status="Reading clipboard…".into();
        std::thread::spawn(move||{
            // Connect failures and a silent owner (500 ms timeout) both fall back to the in-app clipboard.
            let system=Clipboard::connect().ok().and_then(|clipboard|clipboard.get_files().ok().flatten())
                .map(|(paths,op)|(paths,op==FileOp::Cut));
            let resolved=resolve_paste(system,in_app.as_ref(),spent.as_deref());
            if tx.send(WorkerResult::PasteResolved(resolved)).is_ok() {let _=wake.write_all(&[1]);}
        });
    }
    fn apply_paste(&mut self,resolved:Option<(Vec<PathBuf>,bool)>) {
        match resolved {
            Some((sources,cut))=>{
                // A system cut becomes the in-app cut, so completion and partial-retry follow the normal path.
                if cut {self.model.clipboard=Some((sources.clone(),true));}
                self.start_transfer(sources,cut);
            },
            None=>self.model.status="Nothing to paste".into(),
        }
    }
    fn action_menu(&mut self,cmd:&str) {
        if self.busy && ["paste","duplicate","trash","newfolder","rename"].contains(&cmd) {
            self.model.status="File operation already in progress".into();self.context=None;return;
        }
        let selected=self.selected();
        let paths=self.selected_files();
        if paths.len()>1 && ["open","openwith","rename","properties","extract"].contains(&cmd) {
            self.model.status="Select one item for this action".into();
            self.context=None;
            return;
        }
        match cmd {
            "open"=>if let Some(path)=selected {self.open(&path)},
            "openwith"=>if let Some(path)=selected {self.request_open_with(path)},
            "copy"|"cut"=>{
                if !paths.is_empty() {
                    self.model.status=format!("{} {} item(s)",if cmd=="cut"{"Cut"}else{"Copied"},paths.len());
                    self.model.clipboard=Some((paths.clone(),cmd=="cut"));
                    self.publish_system_clipboard(paths,cmd=="cut");
                }
            },
            "paste"=>self.request_paste(),
            "duplicate"=>self.start_transfer(paths,false),
            "newfolder"=>{
                let directory=self.model.tab().path().to_path_buf();
                if self.tx.send(WorkerJob::Create{directory}).is_ok() {
                    self.busy=true;self.model.status="Creating folder…".into();
                }else{self.model.status="File worker unavailable".into();}
            },
            "rename"=>if let Some(path)=selected {self.edit=Some(EditMode::Rename);self.buffer=path.file_name().unwrap_or_default().to_string_lossy().into_owned();},
            "trash"=>if !paths.is_empty() {
                let len=paths.len();
                if self.tx.send(WorkerJob::Trash{paths}).is_ok() {
                    self.busy=true;self.model.status=format!("Moving {len} item(s) to Trash…");
                } else {self.model.status="Trash worker unavailable".into();}
            },
            "terminal"=>self.spawn_terminal(),
            "compress"=>self.compress_selected(),
            "extract"=>if let Some(path)=selected.as_deref() {
                // Ark opens a destination dialog; no implicit overwrites in Explorer.
                self.spawn_ark(&["--batch","--dialog"],path);
            },
            "newtab"=>if self.model.tabs.len()<5 {self.model.new_tab();self.scan();},
            "bookmark"=>{let path=self.model.tab().path().to_path_buf();match add_bookmark(&path,&home_dir()) {Ok(())=>{self.reload_places();self.model.status="Added to Places".into();},Err(e)=>self.model.status=format!("Bookmark failed: {e}")}},
            "properties"=>if let Some(path)=selected {self.open_properties(path)},
            "opencontaining"=>if let Some(path)=selected {self.open_containing(path)},
            "restore"=>if let Some(path)=selected {
                if self.tx.send(WorkerJob::Restore{base:self.trash_base.clone(),stored:path}).is_ok() {
                    self.busy=true;self.model.status="Restoring…".into();
                }
            },
            "emptytrash"=>self.dialog=Some(Dialog::Confirm{
                text:"Permanently empty the Trash? This cannot be undone.".into(),
                pending:Pending::EmptyTrash(self.trash_base.clone()),
            }),
            "placerename"=>if let Some(place)=self.context_place.and_then(|i|self.places.get(i)) {
                self.edit=Some(EditMode::PlaceName);self.buffer=place.name.clone();
            },
            "placeremove"=>if let Some(path)=self.context_place_path() {self.mutate_bookmark(&path,|items,index|bookmarks::remove(items,index).is_some());},
            "hidden"=>self.toggle_hidden(),
                "unmount"|"eject"=>if let Some(place)=self.context_place.and_then(|i|self.places.get(i)).cloned() {
                if let Some(device)=place.device {self.device_action(if cmd=="eject"{"eject"}else{"unmount"},device,place.name);}
            },
            _=>{}
        }
        self.context=None;
        self.context_place=None;
    }
    fn context_place_path(&self)->Option<PathBuf> {
        self.context_place.and_then(|i|self.places.get(i)).map(|p|p.path.clone())
    }
    fn is_bookmark(&self,index:usize)->bool {
        self.places.get(index).is_some_and(|p|p.group==PlaceGroup::Pinned && !p.removable && p.path!=self.trash_base.join("files"))
    }
    /// Edits the GTK bookmarks file atomically; the sidebar is rebuilt from the saved file.
    fn mutate_bookmark(&mut self,path:&Path,edit:impl FnOnce(&mut Vec<bookmarks::Bookmark>,usize)->bool) {
        let config=std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(||home_dir().join(".config"));
        let file=bookmarks::bookmarks_file(&config);
        let mut items=bookmarks::load_bookmarks(&file);
        let Some(index)=items.iter().position(|b|b.path==path) else {self.model.status="Bookmark not found".into();return;};
        if !edit(&mut items,index) {return;}
        match bookmarks::save_bookmarks(&file,&items) {
            Ok(())=>{self.reload_places();self.model.status="Places updated".into();},
            Err(error)=>self.model.status=format!("Places not saved: {error}"),
        }
    }
    /// Mount, unmount or eject a removable device through udisks2 on a worker thread; the outcome is shown in the status strip.
    fn device_action(&mut self,verb:&'static str,device:String,label:String) {
        self.model.status=format!("{} {label}…",match verb {"mount"=>"Mounting","unmount"=>"Unmounting",_=>"Ejecting"});
        self.spawn_background(move||WorkerResult::DeviceDone(match bookmarks::udisks_action(verb,&device) {
            Ok(_)=>Ok(format!("{} {label}",match verb {"mount"=>"Mounted","unmount"=>"Unmounted",_=>"Ejected"})),
            Err(error)=>Err(format!("{verb} {label} failed: {error}")),
        }));
    }
    /// Opens a place, or mounts a removable device that is present but not mounted.
    fn activate_place(&mut self,index:usize) {
        let Some(place)=self.places.get(index).cloned() else {return};
        if place.mounted() {self.navigate(place.path);}
        else if let Some(device)=place.device {self.device_action("mount",device,place.name);}
    }
    /// Sidebar button at the right of a device row: eject when mounted, mount when not.
    fn place_button(&mut self,index:usize) {
        let Some(place)=self.places.get(index).cloned() else {return};
        let Some(device)=place.device.clone() else {return};
        if place.mounted() {self.device_action("eject",device,place.name);} else {self.device_action("mount",device,place.name);}
    }
    fn open_containing(&mut self,path:PathBuf) {
        let Some(parent)=path.parent().map(Path::to_path_buf) else {return};
        self.clear_search();
        self.navigate(parent);
        self.model.tab_mut().select_one(path);
    }
    fn open_properties(&mut self,path:PathBuf) {
        let props=match properties::read(&path) {
            Ok(props)=>props,
            Err(error)=>{self.model.status=format!("Properties: {error}");return;},
        };
        let cancel=Arc::new(AtomicBool::new(false));
        let is_dir=fs::symlink_metadata(&path).is_ok_and(|m|m.is_dir());
        let size=(!is_dir).then_some(props.size);
        if is_dir {
            // Recursive size runs on its own thread so the UI and file worker stay responsive.
            let events=self.events.as_ref().and_then(|(tx,wake)|Some((tx.clone(),wake.try_clone().ok()?)));
            if let Some((tx,mut wake))=events {
                let (flag,target)=(Arc::clone(&cancel),path.clone());
                std::thread::spawn(move||{
                    let bytes=properties::recursive_size(&target,&flag);
                    if tx.send(WorkerResult::SizeReady{path:target,bytes}).is_ok() {let _=wake.write_all(&[1]);}
                });
            }
        }
        self.context=None;
        self.dialog=Some(Dialog::Properties{props,size,cancel});
    }
    fn confirm_delete(&mut self) {
        let paths=self.selected_files();
        if paths.is_empty() {return;}
        self.context=None;
        self.dialog=Some(Dialog::Confirm{
            text:format!("Permanently delete {} item(s)? This cannot be undone.",paths.len()),
            pending:Pending::Delete(paths),
        });
    }
    /// Button slots: primary, secondary, tertiary, cancel. Each dialog kind decides what they mean.
    fn dialog_action(&mut self,slot:&str) {
        // Permissions are edited in the address field; Enter applies, Escape cancels that edit only.
        if let (Some(Dialog::Properties{props,..}),"primary")=(self.dialog.as_ref(),slot) {
            self.buffer=format!("{:o}",props.mode&0o7777);
            self.edit=Some(EditMode::Mode);
            return;
        }
        let Some(dialog)=self.dialog.take() else {return};
        match (dialog,slot) {
            (Dialog::Properties{cancel,..},_)=>cancel.store(true,Ordering::SeqCst),
            (Dialog::Confirm{pending,..},"primary")=>self.run_pending(pending),
            (Dialog::Confirm{..},_)=>{},
            (Dialog::Conflicts{sources,directory,move_it,..},"primary")=>self.send_transfer(sources,directory,move_it,Conflict::Skip),
            (Dialog::Conflicts{sources,directory,move_it,..},"secondary")=>self.send_transfer(sources,directory,move_it,Conflict::KeepBoth),
            (Dialog::Conflicts{sources,directory,move_it,count},"tertiary")=>{
                self.dialog=Some(Dialog::Confirm{
                    text:format!("Replace {count} existing item(s)? Replaced items are not recoverable."),
                    pending:Pending::Transfer{sources,directory,move_it,conflict:Conflict::Replace},
                });
            },
            (Dialog::Conflicts{..},_)=>{},
        }
    }
    fn run_pending(&mut self,pending:Pending) {
        match pending {
            Pending::Delete(paths)=>{
                if self.tx.send(WorkerJob::Delete{paths}).is_ok() {self.busy=true;self.model.status="Deleting…".into();}
            },
            Pending::EmptyTrash(base)=>{
                if self.tx.send(WorkerJob::EmptyTrash{base}).is_ok() {self.busy=true;self.model.status="Emptying Trash…".into();}
            },
            Pending::Transfer{sources,directory,move_it,conflict}=>self.send_transfer(sources,directory,move_it,conflict),
        }
    }
    /// Applies a permission string entered in the Properties address field.
    fn apply_mode(&mut self,text:&str) {
        let Some(Dialog::Properties{props,..})=self.dialog.as_ref() else {return};
        let path=props.path.clone();
        let result=properties::parse_mode(text).and_then(|mode|properties::set_mode(&path,mode).map_err(|e|e.to_string()));
        match result {
            Ok(())=>{
                if let Some(Dialog::Properties{props,..})=self.dialog.as_mut() {
                    if let Ok(fresh)=properties::read(&path) {*props=fresh;}
                }
                self.model.status="Permissions changed".into();
                self.scan();
            },
            Err(error)=>self.model.status=format!("Permissions not changed: {error}"),
        }
    }
    fn event(&mut self,e:&UiActionEvent) {
        let action=e.action.as_str();
        if self.chooser_loading || self.chooser.is_some() {
            if e.phase==UiActionPhase::Release && e.inside {
                if let Some(action)=action.strip_prefix("explorer.chooser.") {
                    self.chooser_action(action);
                }
            }
            return;
        }
        // Dialogs are modal: only their own buttons and the progress Cancel react.
        if self.dialog.is_some() {
            if let Some(slot)=action.strip_prefix("explorer.dialog.") {
                if e.phase==UiActionPhase::Release && e.inside {self.dialog_action(slot);}
            }
            return;
        }
        if let Some(slot)=action.strip_prefix("explorer.row.").and_then(|v|v.parse::<usize>().ok()) {
            match e.phase {
                UiActionPhase::Press if e.button==1 && e.inside && !e.shift_down()
                    && !e.ctrl_down() && !self.busy => {
                    if let Some(entry)=self.row_entry(slot) {
                        self.row_drag=Some(RowDrag::new(entry.path.clone(),e.x,e.y));
                    }
                    return;
                },
                UiActionPhase::Motion => {
                    let active=self.row_drag.as_mut().is_some_and(|drag|drag.motion(e.x,e.y));
                    if active {
                        self.drop_folder=self.folder_at(e.x,e.y);
                    }
                    return;
                },
                UiActionPhase::Release => {
                    if self.finish_row_drag(e) {return;}
                },
                _=>{},
            }
        }
        // Wheel notches arrive as the render layer's "scroll" action; text is "dx,dy" with dy<0 for up.
        if action=="scroll" {
            let dy=e.text.as_deref().and_then(|t|t.split_once(',')).and_then(|(_,dy)|dy.parse::<f32>().ok()).unwrap_or(0.0);
            if e.inside && dy!=0.0 && (SIDEBAR_WIDTH..self.window.0).contains(&e.x) {
                let rows=self.visible_rows();
                self.model.tab_mut().scroll_by(dy.signum() as isize*WHEEL_ROWS,rows);
            }
            return;
        }
        if action=="explorer.surface" {
            if e.button==3 && e.phase==UiActionPhase::Release && e.inside {
                self.context_on_entry=false;
                self.context=Some((e.x,e.y));self.menu_overflow=false;
                self.rubber=None;
            } else {
                match e.phase {
                    UiActionPhase::Press if e.button==1 && e.inside => {
                        self.context=None;
                        self.rubber=Some(((e.x,e.y),(e.x,e.y)));
                    },
                    UiActionPhase::Motion => if let Some((start,_))=self.rubber {
                        self.rubber=Some((start,(e.x,e.y)));
                    },
                    UiActionPhase::Release => {
                        if let Some(((x0,y0),(x1,y1)))=self.rubber.take() {
                            let top=list_top(self.model.tabs.len()>1);
                            if (x1-x0).abs()>5.0 || (y1-y0).abs()>5.0 {
                                let start=((y0.min(y1)-top)/ROW_HEIGHT).floor().max(0.0) as usize;
                                let end=((y0.max(y1)-top)/ROW_HEIGHT).floor().max(0.0) as usize;
                                let rows=self.visible_rows();
                                self.model.tab_mut().select_vertical_band(start.min(rows),end.min(rows),rows);
                            } else {
                                self.model.tab_mut().clear_selection();
                            }
                        }
                    },
                    _=>{},
                }
            }
            return;
        }
        if e.phase!=UiActionPhase::Release || !e.inside {return;}
        if let Some(slot)=action.strip_prefix("explorer.row.").and_then(|v|v.parse::<usize>().ok()) {
            if let Some(entry)=self.row_entry(slot).cloned() {
                let path=entry.path;
                if e.button == 3 {
                    // Context-click on a selected file preserves the full batch selection.
                    if !self.model.tab().selected_paths.contains(&path) {
                        self.model.tab_mut().select_one(path.clone());
                    }
                    self.last_click=None;
                    self.context_on_entry=true;
                    self.context_place=None;
                    self.context=Some((e.x,e.y));self.menu_overflow=false;
                } else if e.button == 1 && e.shift_down() {
                    // Shift selects a contiguous range using the existing anchor.
                    // Ctrl+Shift adds that range to the current selection.
                    self.model.tab_mut().select_range_to_with_mode(path,e.ctrl_down());
                    self.last_click=None;
                    self.context=None;
                } else if e.button == 1 && e.ctrl_down() {
                    self.model.tab_mut().toggle_selection(path);
                    self.last_click=None;
                    self.context=None;
                } else if e.button == 1 {
                    let double_click=self.model.tab().selected_paths.len()==1
                        && self.last_click.as_ref().is_some_and(|(prev,t)|
                            prev==&path && t.elapsed()<Duration::from_millis(500));
                    self.model.tab_mut().select_one(path.clone());
                    if double_click {self.last_click=None;self.open(&path);}
                    else {self.context=None;self.last_click=Some((path,Instant::now()));}
                }
            }
            return;
        }
        if let Some(row)=action.strip_prefix("explorer.place.").and_then(|v|v.parse::<usize>().ok()) {
            // `row` is a sidebar slot; headings have no place and ignore clicks.
            if let Some(SidebarRow::Place(index))=sidebar_rows(&self.places).get(row).copied() {
                if e.button==3 {
                    // Right-click on a sidebar item opens the place menu instead of navigating.
                    self.context_on_entry=false;
                    self.context_place=Some(index);
                    self.context=Some((e.x,e.y));self.menu_overflow=false;
                } else {self.activate_place(index);}
            }
            return;
        }
        if let Some(row)=action.strip_prefix("explorer.placeact.").and_then(|v|v.parse::<usize>().ok()) {
            if let Some(SidebarRow::Place(index))=sidebar_rows(&self.places).get(row).copied() {self.place_button(index);}
            return;
        }
        if let Some(slot)=action.strip_prefix("explorer.tab.").and_then(|v|v.parse::<usize>().ok()) {
            if self.model.switch_tab(slot) {self.scan();} return;
        }
        if let Some(action)=action.strip_prefix("explorer.menu.") {self.action_menu(action);return;}
        match action {
            "explorer.back"=>{if self.model.tab_mut().back(){self.scan();}},
            "explorer.forward"=>{if self.model.tab_mut().forward(){self.scan();}},
            "explorer.up"=>{if self.model.tab_mut().up(){self.scan();}},
            "explorer.reload"=>{self.reload_places();self.refresh_devices();self.scan();},
            "explorer.hidden"=>self.toggle_hidden(),
            "explorer.search"=>{self.edit=Some(EditMode::Search);self.buffer=self.model.tab().search.clone();},
            "explorer.search.clear"=>{self.edit=None;self.clear_search();self.search_text.clear();self.model.status="Search cleared".into();},
            "explorer.address"=>{self.edit=Some(EditMode::Path);self.buffer=self.model.tab().path().to_string_lossy().into_owned();},
            "explorer.tab.new"=>{if self.model.tabs.len()<5 {self.model.new_tab();self.scan();}},
            "explorer.tab.close"=>{self.model.close_tab();self.scan();},
            "explorer.sort.name"=>self.sort_by(Column::Name),
            "explorer.sort.size"=>self.sort_by(Column::Size),
            "explorer.sort.modified"=>self.sort_by(Column::Modified),
            "explorer.sort.kind"=>self.sort_by(Column::Type),
            "explorer.menu"=>{self.context_on_entry=false;self.menu_overflow=true;self.context=Some((self.window.0-8.0,53.0));},
            "explorer.cancel"=>if self.busy {self.cancel.cancel();self.model.status="Cancelling…".into();},
            _=>{}
        }
    }
    fn key(&mut self,key:&UiKeyEvent) {
        if key.phase!=UiKeyPhase::Press {return;}
        let name=key.key.to_lowercase();
        if self.chooser_loading || self.chooser.is_some() {
            match name.as_str() {
                "escape"=>self.dismiss_chooser(),
                "return"|"enter"=>self.launch_chooser_choice(false),
                "up"|"down"=>if let Some(chooser)=self.chooser.as_mut() {
                    let indexes=chooser.page_choices().iter().map(|(index,_)|*index).collect::<Vec<_>>();
                    if !indexes.is_empty() {
                        let position=indexes.iter().position(|index|Some(*index)==chooser.selected).unwrap_or(0);
                        let next=if name=="up"{position.saturating_sub(1)}else{(position+1).min(indexes.len()-1)};
                        chooser.selected=Some(indexes[next]);
                    }
                },
                _=>{}
            }
            return;
        }
        let ctrl=key.modifiers.iter().any(|m|m.eq_ignore_ascii_case("ctrl")||m.eq_ignore_ascii_case("control"));
        let alt=key.modifiers.iter().any(|m|m.eq_ignore_ascii_case("alt"));
        let shift=key.modifiers.iter().any(|m|m.eq_ignore_ascii_case("shift"));
        if let Some(edit)=self.edit {
            if name=="escape" {self.edit=None;return;}
            if name=="return"||name=="enter" {
                let content=std::mem::take(&mut self.buffer);self.edit=None;
                match edit {
                    EditMode::Path=>match parse_address(&content,&home_dir()) {
                        Ok(AddressTarget::Local(path))=>self.navigate(path),
                        Ok(AddressTarget::ExternalUri(uri))=>{
                            match Command::new("xdg-open").arg(&uri).spawn() {
                                Ok(_)=>self.model.status="Remote URL passed to the installed system handler".into(),
                                Err(e)=>self.model.status=format!("Remote handler unavailable: {e}"),
                            }
                        },
                        Err(reason)=>self.model.status=reason,
                    },
                    EditMode::Search=>self.start_search(content),
                    EditMode::Mode=>self.apply_mode(&content),
                    EditMode::PlaceName=>{
                        let path=self.context_place_path();
                        self.context_place=None;
                        if let Some(path)=path {self.mutate_bookmark(&path,|items,index|bookmarks::rename(items,index,&content));}
                    },
                    EditMode::Rename=>if let Some(path)=self.selected() {
                        match flamewm_desktop_core::file_actions::validate_rename_name(&content) {
                            Ok(name)=>{
                                if self.busy {self.model.status="File operation already in progress".into();}
                                else {
                                    let to=path.with_file_name(name);
                                    if self.tx.send(WorkerJob::Rename{from:path,to}).is_ok() {
                                        self.busy=true;self.model.status="Renaming…".into();
                                    }else{self.model.status="File worker unavailable".into();}
                                }
                            },
                            Err(e)=>self.model.status=e.to_string(),
                        }
                    },
                }
                return;
            }
            if name=="backspace" {self.buffer.pop();return;}
            if let Some(text)=&key.text {if !ctrl && !alt && !text.chars().any(char::is_control) {self.buffer.push_str(text);}}
            return;
        }
        if let Some(text)=key.text.as_deref().filter(|t|!ctrl && !alt && t.chars().count()==1 && !t.chars().any(char::is_control)) {
            self.typeahead(text);
            return;
        }
        // Dialogs swallow keys; Enter confirms the primary button and Escape cancels.
        if self.dialog.is_some() {
            match name.as_str() {
                "escape"=>self.dialog_action("cancel"),
                "return"|"enter"=>self.dialog_action("primary"),
                _=>{},
            }
            return;
        }
        match name.as_str() {
            "f5"=>self.scan(),
            "f2"=>self.action_menu("rename"),
            "delete" if shift=>self.confirm_delete(),
            "delete"=>self.action_menu("trash"),
            "return"|"enter" if alt=>if let Some(path)=self.selected(){self.open_properties(path)},
            "backspace"=>{if self.model.tab_mut().up(){self.scan();}},
            "escape"=>{if self.busy {self.cancel.cancel();self.model.status="Cancelling…".into();return;}if self.search_rows.is_some() {self.clear_search();self.model.status="Search cleared".into();return;}self.context=None;self.rubber=None;self.row_drag=None;self.drop_folder=None;self.last_click=None;self.model.tab_mut().clear_selection();},
            "return"|"enter"=>if let Some(path)=self.selected(){self.open(&path)},
            "left" if alt=>{if self.model.tab_mut().back(){self.scan();}},
            "right" if alt=>{if self.model.tab_mut().forward(){self.scan();}},
            "l" if ctrl=>{self.edit=Some(EditMode::Path);self.buffer=self.model.tab().path().to_string_lossy().into_owned();},
            "f" if ctrl=>{self.edit=Some(EditMode::Search);self.buffer=self.model.tab().search.clone();},
            "t" if ctrl=>{if self.model.tabs.len()<5{self.model.new_tab();self.scan();}},
            "w" if ctrl=>{self.model.close_tab();self.scan();},
            "h" if ctrl=>self.toggle_hidden(),
            "a" if ctrl=>self.model.tab_mut().select_all_filtered(),
            "space"|" " if ctrl=>if let Some(path)=self.selected() {self.model.tab_mut().toggle_selection(path)},
            "c" if ctrl=>self.action_menu("copy"),
            "x" if ctrl=>self.action_menu("cut"),
            "v" if ctrl=>self.action_menu("paste"),
            "z" if ctrl&&shift=>self.history_last(false),
            "z" if ctrl=>self.history_last(true),
            "y" if ctrl=>self.history_last(false),
            "up"=>self.step_selection(-1,shift),
            "down"=>self.step_selection(1,shift),
            // PageUp, PageDown, Home and End have no named keysym in the UI key table, so they arrive as Keysym(0x...).
            "keysym(0xff55)"=>self.step_selection(-(self.visible_rows() as isize),shift),
            "keysym(0xff56)"=>self.step_selection(self.visible_rows() as isize,shift),
            "keysym(0xff50)"=>self.select_index(0,shift),
            "keysym(0xff57)"=>{let count=self.model.tab().filtered_count();if count>0{self.select_index(count-1,shift);}},
            _=>{}
        }
    }
    fn step_selection(&mut self,delta:isize,extend:bool){
        let tab=self.model.tab();
        let count=tab.filtered_count();
        if count==0{return;}
        let current=self.selected().as_ref().and_then(|p|tab.visible_index_of(p)).unwrap_or(0);
        let next=(current as isize+delta).clamp(0,count as isize-1) as usize;
        self.select_index(next,extend);
    }
    /// Select the filtered row at `index` and scroll the window just enough to show it.
    fn select_index(&mut self,index:usize,extend:bool){
        let rows=self.visible_rows();
        let tab=self.model.tab();
        let Some(path)=tab.visible_nth(index).map(|entry|entry.path.clone()) else {return};
        let tab=self.model.tab_mut();
        if extend {tab.select_range_to(path)}else{tab.select_one(path)}
        tab.reveal(index,rows);
    }
    fn wake(&mut self,event:WorkerResult){
        match event {
            WorkerResult::ChooserReady{request,chooser}=>{
                if request!=self.chooser_request || !self.chooser_loading {return;}
                self.chooser_loading=false;
                match chooser {
                    Ok(chooser)=>self.chooser=Some(chooser),
                    Err(error)=>self.model.status=format!("Open With: {error}"),
                }
            },
            WorkerResult::DefaultSaved(outcome)=>{
                if let Err(error)=outcome {self.model.status=format!("MIME default was not saved: {error}");}
            },
            WorkerResult::Scanned{generation,path,entries}=>{
                let vanished=entries.is_err() && !path.exists();
                if self.model.apply_listing(generation,&path,entries) {
                    view::sort_entries(&mut self.model.tab_mut().entries,self.column_sort);
                }
                // A removed device or folder falls back to Home with a visible notice.
                if vanished && path==*self.model.tab().path() {
                    let home=home_dir();
                    self.activity=Some(format!("{} is no longer available; opened Home",path.display()));
                    self.model.tab_mut().visit(home);
                    self.scan();
                }
            },
            WorkerResult::DirChanged=>{
                if !self.busy && self.dialog.is_none() {self.request_listing();}
            },
            WorkerResult::ScrollTick=>self.step_scroll(),
            WorkerResult::SearchTick=>self.take_search_results(),
            WorkerResult::SizeReady{path,bytes}=>{
                if let Some(Dialog::Properties{props,size,..})=self.dialog.as_mut() {
                    if props.path==path {*size=Some(bytes.unwrap_or(0));}
                }
            },
            WorkerResult::Devices(devices)=>{
                if self.unmounted!=devices {self.unmounted=devices;self.reload_places();}
            },
            WorkerResult::DeviceDone(outcome)=>{
                let message=outcome.unwrap_or_else(|error|error);
                self.model.status=message.clone();
                self.activity=Some(message);
                self.reload_places();
                self.refresh_devices();
            },
            WorkerResult::Progress(bytes)=>{self.progress=Some(bytes);},
            WorkerResult::Total(bytes)=>{self.progress_total=bytes;},
            WorkerResult::BatchCompleted{report,action,cut_sources}=>{
                self.busy=false;
                self.cancel=CancellationToken::new();
                if let Some(sources)=cut_sources.as_ref() {
                    // Partial moves keep only the failed sources for a safe retry; the system list is spent.
                    self.model.clipboard=settle_cut(self.model.clipboard.take(),sources,report.failed_sources.clone());
                    self.system_spent=Some(sources.clone());
                }
                self.progress=None;
                if report.completed>0 {self.scan();}
                let summary=report.message(action);
                self.activity=Some(summary.clone());
                self.model.status=summary;
                if report.completed>0 {
                    let title=if report.failed>0 {"File operation partially completed"}else{"File operation completed"};
                    let _=Command::new("notify-send").arg("--app-name=FlameWM Explorer")
                        .arg(title).arg(format!("{} succeeded, {} failed",report.completed,report.failed)).spawn();
                }
            }
            WorkerResult::PasteResolved(resolved)=>self.apply_paste(resolved),
            WorkerResult::Completed(result)=>{
                self.busy=false;
                let good=result.is_ok();
                let message=result.unwrap_or_else(|e|e);
                self.model.status=message.clone();
                self.progress=None;
                if good {
                    if let Err(e)=Command::new("notify-send").arg("--app-name=FlameWM Explorer").arg("File operation completed").spawn() {
                        eprintln!("flamewm-explorer: notification unavailable: {e}");
                    }
                    self.scan();
                }
                self.activity=Some(message);
            }
        }
    }
    fn project(&self, doc:&mut impl UiDocumentAccess)->Result<(),String>{
        self.theme.project(doc)?;
        let tab=self.model.tab();
        // The path bar always shows the folder; search text belongs to the search field only.
        let address=match self.edit {Some(EditMode::Path)=>format!("{}|",self.buffer),Some(EditMode::Rename)=>format!("Rename: {}|",self.buffer),Some(EditMode::Mode)=>format!("Permissions: {}|",self.buffer),Some(EditMode::PlaceName)=>format!("Place name: {}|",self.buffer),_=>display_location(tab.path(),&home_dir())};
        doc.text("address-text",address)?;
        // The field shows only the query: a plain placeholder when empty, and the clear button while a search is active.
        let search=match self.edit {
            Some(EditMode::Search)=>format!("{}|",self.buffer),
            _ if self.search_rows.is_some()=>self.search_text.clone(),
            _=>"Search".to_owned(),
        };
        doc.text("search-text",search)?;
        doc.visible("search-clear",self.search_rows.is_some())?;
        doc.text("menu-hidden",format!("Show Hidden Files: {}",if self.model.hidden {"On"} else {"Off"}))?;
        doc.foreground("nav-back",if tab.can_back(){WHITE}else{self.theme.muted()})?;
        doc.foreground("nav-forward",if tab.can_forward(){WHITE}else{self.theme.muted()})?;
        // Native Finder-style chrome: a single tab has no visible tab strip.
        let tab_strip=self.model.tabs.len()>1;
        doc.visible("tabs",tab_strip)?;
        for i in 0..5 {
            let exists=tab_strip && i<self.model.tabs.len();
            doc.visible(&format!("tab-{i}"),exists)?;
            if exists {
                let title=self.model.tabs[i].path().file_name().unwrap_or_default().to_string_lossy().into_owned();
                doc.text(&format!("tab-{i}"),if title.is_empty(){"System".into()}else{title})?;
                doc.background(&format!("tab-{i}"),if i==self.model.active{self.theme.selected()}else{self.theme.card()})?;
            }
        }
        let rows=sidebar_rows(&self.places);
        for slot in 0..SIDEBAR_SLOTS {
            // Each slot shows either a group heading or a place button; the other element stays hidden.
            match rows.get(slot) {
                Some(SidebarRow::Heading(title)) => {
                    doc.visible(&format!("sb-head-{slot}"),true)?;
                    doc.text(&format!("sb-head-label-{slot}"),(*title).to_owned())?;
                    doc.visible(&format!("prow-{slot}"),false)?;
                    doc.visible(&format!("place-{slot}"),false)?;
                }
                Some(SidebarRow::Place(index)) => {
                    let place=&self.places[*index];
                    let selected=!place.path.as_os_str().is_empty() && place.path==tab.path();
                    let unmounted=!place.mounted();
                    doc.visible(&format!("sb-head-{slot}"),false)?;
                    doc.visible(&format!("prow-{slot}"),true)?;
                    doc.visible(&format!("place-{slot}"),true)?;
                    for kind in PLACE_ICONS {doc.visible(&format!("pico-{slot}-{kind}"),kind==place.icon)?;}
                    doc.text(&format!("place-label-{slot}"),place.name.clone())?;
                    doc.foreground(&format!("place-label-{slot}"),if unmounted && place.device.is_some() {self.theme.muted()} else {WHITE})?;
                    doc.background(&format!("place-{slot}"),if selected {self.theme.selected()} else {self.theme.clear()})?;
                    // Device rows get a Dolphin-style button: eject while mounted, mount while not.
                    let device=place.device.is_some();
                    doc.visible(&format!("pact-{slot}"),device)?;
                    doc.visible(&format!("pact-eject-{slot}"),device && place.mounted())?;
                    doc.visible(&format!("pact-mount-{slot}"),device && unmounted)?;
                }
                None => {
                    doc.visible(&format!("sb-head-{slot}"),false)?;
                    doc.visible(&format!("prow-{slot}"),false)?;
                    doc.visible(&format!("place-{slot}"),false)?;
                }
            }
        }
        // Only Details shows the metadata columns; Icons shows names alone.
        // The header and every row share one geometry: flow cells with identical fixed widths.
        doc.position("columns",SIDEBAR_WIDTH,header_top(tab_strip))?;
        doc.size("columns",self.window.0-SIDEBAR_WIDTH,HEADER_HEIGHT)?;
        doc.visible("columns",true)?;
        // The engine has no flex-shrink, so a tall sidebar would push the status strip out of the window.
        // The body gets an explicit height: window minus toolbar/tabs, the two 1px hairlines and the status strip.
        doc.size("body-area",self.window.0,(self.window.1-header_top(tab_strip)-2.0-STATUS_STRIP_HEIGHT).max(0.0))?;
        // Sort direction is a drawn triangle per column; only the active column shows one.
        for (column,key) in [(Column::Name,"name"),(Column::Modified,"mod"),(Column::Size,"size"),(Column::Type,"kind")] {
            let active=self.column_sort.column==column;
            doc.visible(&format!("sort-{key}-asc"),active&&!self.column_sort.descending)?;
            doc.visible(&format!("sort-{key}-desc"),active&&self.column_sort.descending)?;
        }
        // List rows glide: slot k draws window row k-1 at its eased offset; the outer slots are off-window until they settle.
        let rows=self.visible_rows();
        let origin=self.origin(rows);
        let top=list_top(self.model.tabs.len()>1);
        let bottom=self.window.1-STATUS_STRIP_HEIGHT;
        for slot in 0..ROW_SLOTS {
            let row=format!("row-{slot}");
            let index=flamewm_explorer::row_slot_index(origin,slot);
            let y=top+(slot as f32-1.0+self.scroll_offset)*ROW_HEIGHT;
            let entry=if slot<=rows+1 {self.entry_at(index)} else {None};
            if let (Some(entry),true)=(entry,y>=top-0.5&&y+ROW_HEIGHT<=bottom+0.5) {
                doc.visible(&row,true)?;
                doc.position(&row,SIDEBAR_WIDTH,y)?;
                doc.size(&row,self.window.0-SIDEBAR_WIDTH,ROW_HEIGHT)?;
                doc.text(&format!("name-{slot}"),entry.name.clone())?;
                doc.text(&format!("size-{slot}"),entry.size_label())?;
                doc.text(&format!("modified-{slot}"),entry.modified_label())?;
                doc.text(&format!("kind-{slot}"),view::kind_name(entry))?;
                // Dot files show the dimmed name twin (`name-dim-*`, CSS colour). Foreground overrides cannot reach a
                // text child, and the icon raster is shared, so the icon stays full strength.
                let dim=entry.name.starts_with('.');
                doc.visible(&format!("name-{slot}"),!dim)?;
                doc.visible(&format!("name-dim-{slot}"),dim)?;
                doc.text(&format!("name-dim-{slot}"),entry.name.clone())?;
                let icon=view::file_icon(entry);
                for kind in ROW_ICONS {doc.visible(&format!("ico-{slot}-{kind}"),kind==icon)?;}
                doc.background(&row,if self.drop_folder.as_deref()==Some(entry.path.as_path()) {
                    self.theme.selected()
                } else if tab.selected_paths.contains(&entry.path){self.theme.selected()}
                    else if index%2==0{self.theme.clear()}else{self.theme.row_alternate()})?;
            } else {doc.visible(&row,false)?;}
        }
        if let Some(((x0,y0),(x1,y1)))=self.rubber {
            let width=(x1-x0).abs();let height=(y1-y0).abs();
            let visible=width>5.0 || height>5.0;
            doc.visible("selection-rectangle",visible)?;
            if visible {
                doc.position("selection-rectangle",x0.min(x1),y0.min(y1))?;
                doc.size("selection-rectangle",width.max(1.0),height.max(1.0))?;
                doc.background("selection-rectangle",self.theme.selection_fill())?;
            }
        } else {doc.visible("selection-rectangle",false)?;}
        let status=if self.row_drag.as_ref().is_some_and(|drag|drag.dragging) {
            if let Some(folder)=self.drop_folder.as_deref() {
                format!("Drop into {} · Move (Ctrl to copy)",folder.file_name().unwrap_or_default().to_string_lossy())
            } else {"Drag onto a folder to move · Hold Ctrl to copy".into()}
        } else {self.activity.as_ref().unwrap_or(&self.model.status).clone()};
        doc.text("status-message",status)?;
        doc.text("status-count",view::items_text(tab.entries.len(),tab.selected_paths.len()))?;
        // Free space and usage come from statvfs; the bar fills by the share of the filesystem in use.
        let space=view::fs_space(tab.path());
        doc.text("status-free",space.map_or(String::new(),|(free,_)|format!("{} left",view::human_size(free))))?;
        let used=space.map_or(0.0,|(free,total)|if total>0 {1.0-free as f32/total as f32} else {0.0});
        doc.size("usage-fill",USAGE_TRACK_WIDTH*used.clamp(0.0,1.0),4.0)?;
        // Determinate when the worker counted the batch; otherwise the fill stays empty.
        let total=self.progress_total;
        let done=self.progress.unwrap_or(0);
        let fraction=if total>0 {(done as f32/total as f32).min(1.0)} else {0.0};
        let progress_text=match self.progress {
            Some(_) if total>0 =>format!("{} of {}",view::human_size(done.min(total)),view::human_size(total)),
            Some(_) =>format!("{} transferred",view::human_size(done)),
            None=>String::new(),
        };
        doc.text("job-progress",progress_text.clone())?;
        doc.visible("progress-popup",self.progress.is_some())?;
        doc.text("progress-popup-copy",progress_text)?;
        doc.size("progress-fill",262.0*fraction,7.0)?;
        doc.visible("context-popup",self.context.is_some() && !self.chooser_loading && self.chooser.is_none())?;
        self.project_chooser(doc)?;
        self.project_dialog(doc)?;
        let selected_path=self.selected();
        let mut visible_rows=0;
        let many=self.model.tab().selected_paths.len()>1;
        let in_trash=tab.path()==self.trash_base.join("files").as_path();
        let searching=self.search_rows.is_some();
        for item in MENU_ITEMS {
            let enabled=if self.context_place.is_some() {
                // Sidebar menu: bookmarks can be renamed or removed, mounted devices unmounted or ejected.
                match item {
                    "placerename"|"placeremove"=>self.context_place.is_some_and(|i|self.is_bookmark(i)),
                    "unmount"|"eject"=>self.context_place.is_some_and(|i|self.places.get(i).is_some_and(|p|p.removable && p.mounted())),
                    _=>false,
                }
            } else {match item {
                "terminal"|"newtab"|"newfolder"|"bookmark"=>true,
                "paste"=>true,
                "emptytrash"=>in_trash,
                "extract"=>self.ark && self.context_on_entry && !many && selected_path.as_deref().is_some_and(is_archive),
                "compress"=>self.ark && self.context_on_entry && selected_path.is_some(),
                "copy"|"cut"|"trash"|"duplicate"=>self.context_on_entry && selected_path.is_some(),
                "open"|"openwith"|"rename"|"properties"=>self.context_on_entry && !many && selected_path.is_some(),
                "opencontaining"=>searching && self.context_on_entry && !many && selected_path.is_some(),
                "restore"=>in_trash && self.context_on_entry && selected_path.is_some(),
                "hidden"=>self.menu_overflow,
                _=>false,
            }};
            doc.visible(&format!("menu-{item}"),enabled)?;
            if enabled {visible_rows+=1;}
        }
        if let Some((x,y))=self.context {
            // Shared desktop context-menu metrics; hidden actions consume no space.
            let metrics=flamewm_ui_core::context_menu::MenuMetrics::canonical();
            let height=visible_rows as f32*metrics.row_height as f32+2.0*metrics.padding as f32;
            // The overflow menu hangs from the right edge of its button; other menus open at the pointer.
            let left=if self.menu_overflow {x-210.0} else {x};
            doc.position("context-popup",left.min(self.window.0-210.0).max(0.0),y.min(self.window.1-height).max(0.0))?;
        }
        Ok(())
    }
    /// One shared modal: four text lines and four fixed-slot buttons, labelled per dialog kind.
    fn project_dialog(&self, doc:&mut impl UiDocumentAccess)->Result<(),String> {
        let Some(dialog)=self.dialog.as_ref() else {
            doc.visible("dialog",false)?;
            return Ok(());
        };
        doc.visible("dialog",true)?;
        doc.background("dialog",self.theme.menu())?;
        let (title,lines,buttons):(String,Vec<String>,[Option<&str>;4])=match dialog {
            Dialog::Properties{props,size,..}=>(
                props.path.file_name().unwrap_or(props.path.as_os_str()).to_string_lossy().into_owned(),
                vec![
                    props.path.display().to_string(),
                    format!("Size: {}",size.map_or("calculating…".to_owned(),view::human_size)),
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
            Dialog::Confirm{text,..}=>("Confirm".into(),vec![text.clone()],[Some("Confirm"),None,None,Some("Cancel")]),
        };
        doc.text("dialog-title",title)?;
        for (i,line) in (0..4).zip(lines.into_iter().chain(std::iter::repeat(String::new()))) {
            doc.text(&format!("dialog-line-{i}"),line)?;
        }
        for (i,label) in buttons.iter().enumerate() {
            let id=format!("dialog-b{}",i+1);
            doc.visible(&id,label.is_some())?;
            if let Some(label)=label {doc.text(&id,(*label).to_owned())?;}
        }
        Ok(())
    }
    fn project_chooser(&self, doc:&mut impl UiDocumentAccess)->Result<(),String> {
        let showing=self.chooser_loading || self.chooser.is_some();
        doc.visible("openwith-dialog",showing)?;
        if !showing{return Ok(());}
        doc.background("openwith-dialog",self.theme.menu())?;
        doc.background("openwith-toolbar",self.theme.card())?;
        if let Some(chooser)=self.chooser.as_ref() {
            let filename=chooser.path.file_name().unwrap_or_default().to_string_lossy();
            doc.text("openwith-title",format!("Open {filename} with"))?;
            doc.text("openwith-mime",format!("{} · {} compatible apps",chooser.mime,
                chooser.apps.iter().filter(|app|app.associated).count()))?;
            let choices=chooser.page_choices();
            for slot in 0..CHOICE_ROWS {
                let id=format!("openwith-choice-{slot}");
                if let Some((index,choice))=choices.get(slot) {
                    doc.visible(&id,true)?;
                    let suffix=if choice.is_default {" · Default"} else if !choice.associated {" · Other app"} else {""};
                    doc.text(&id,format!("{}{suffix}",choice.entry.name()))?;
                    doc.background(&id,if chooser.selected==Some(*index){self.theme.selected()}else{self.theme.card()})?;
                }else{doc.visible(&id,false)?;}
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
            for slot in 0..CHOICE_ROWS{doc.visible(&format!("openwith-choice-{slot}"),false)?;}
            for id in ["openwith-prev","openwith-next","openwith-all","openwith-once","openwith-always"] {doc.visible(id,false)?;}
            doc.text("openwith-page","".into())?;
        }
        Ok(())
    }
}
const WHITE:UiColor=UiColor{r:240,g:242,b:245,a:255};

/// Live settings at startup; `None` when the control service is unavailable (defaults apply).
fn initial_settings()->Option<SettingsSnapshot> {
    let client=ControlClient::connect(BusKind::Session).ok()?;
    match client.call_with_timeout(&ControlRequest::GetSettings,Duration::from_millis(300)) {
        Ok(ControlResponse::Settings(snapshot))=>Some(snapshot),
        _=>None,
    }
}
/// Packaged asset root for the default wallpaper: the installed share directory, else the source tree (same rule as the desktop).
fn packaged_root()->PathBuf {
    let source=Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    let installed=std::env::current_exe().ok().and_then(|exe|{
        let bin=exe.parent()?;let usr=bin.parent()?;
        (bin.file_name()?=="bin"&&usr.file_name()?=="usr").then(||usr.join("share/flamewm"))
    });
    installed.filter(|root|root.join("assets").is_dir()).unwrap_or(source)
}
fn main(){if let Err(error)=run(){eprintln!("flamewm-explorer: {error}");std::process::exit(1);}}
fn run()->Result<(),String>{
    let args=std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.iter().any(|a|matches!(a.to_string_lossy().as_ref(),"--help"|"-h")) {println!("FlameWM Explorer — usage: flamewm-explorer [DIRECTORY]");return Ok(());}
    let path=args.first().map(PathBuf::from).unwrap_or_else(home_dir);
    let directory=if path.is_dir(){path}else if path.is_file(){path.parent().unwrap_or(Path::new("/")).to_path_buf()}else{return Err(format!("not a directory: {}",path.display()));};
    let directory=fs::canonicalize(&directory).map_err(|e|e.to_string())?;
    let (tx,jobs)=mpsc::channel();
    let (scan_tx,scans)=mpsc::channel();
    let (result_tx,results)=mpsc::channel();
    let (chooser_tx,chooser_jobs)=mpsc::channel();
    let (mut reader,writer)=UnixStream::pair().map_err(|e|e.to_string())?;
    reader.set_nonblocking(true).map_err(|e|e.to_string())?;
    let scan_wake=writer.try_clone().map_err(|e|e.to_string())?;
    let chooser_wake=writer.try_clone().map_err(|e|e.to_string())?;
    let chooser_results=result_tx.clone();
    std::thread::Builder::new().name("flamewm-explorer-mime".into())
        .spawn(move||chooser_worker(chooser_jobs,chooser_results,chooser_wake))
        .map_err(|e|e.to_string())?;
    let scan_results=result_tx.clone();
    std::thread::Builder::new().name("flamewm-explorer-scan".into()).spawn(move||scan_worker(scans,scan_results,scan_wake)).map_err(|e|e.to_string())?;
    // Background threads (search, watcher, recursive size) wake the UI through the same socket.
    let app_events=(result_tx.clone(),writer.try_clone().map_err(|e|e.to_string())?);
    let (search_tx,search_wake)=(result_tx.clone(),writer.try_clone().map_err(|e|e.to_string())?);
    let search=SearchWorker::spawn(move||{if search_tx.send(WorkerResult::SearchTick).is_ok() {let _=(&search_wake).write_all(&[1]);}}).map_err(|e|e.to_string())?;
    std::thread::Builder::new().name("flamewm-explorer-io".into()).spawn(move||worker(jobs,result_tx,writer)).map_err(|e|e.to_string())?;
    let (window_w,window_h,window_y)=window_frame(flamewm_ui_x11::root_geometry().unwrap_or((u32::MAX,u32::MAX)));
    let app=Rc::new(RefCell::new(ExplorerApp::new(directory,tx,scan_tx,chooser_tx)));
    let backdrop:Option<flamewm_ui_x11::backdrop::WindowBackdrop>;
    {
        let mut app=app.borrow_mut();
        app.window=(window_w as f32,window_h as f32);
        app.events=Some(app_events);
        app.search=Some(search);
        let settings=initial_settings();
        app.theme=settings.as_ref().map_or_else(ExplorerTheme::default,ExplorerTheme::from_snapshot);
        backdrop=settings.as_ref().map(|snapshot|theme::window_backdrop(snapshot,&packaged_root()));
        app.model.hidden=prefs::load(&prefs::config_dir()).show_hidden;
    }
    app.borrow_mut().scan();
    // Present-but-unmounted devices arrive from lsblk on a worker; without this they only show after a reload.
    app.borrow().refresh_devices();
    let mut doc=decode_document(UI)?;
    // Translucent surfaces show the blurred wallpaper through the window; opaque (100%) installs nothing.
    if let Some(spec)=backdrop {doc=doc.with_window_backdrop(spec);}
    app.borrow().project(&mut doc)?;
    let event_app=Rc::clone(&app);
    let wake_app=Rc::clone(&app);
    let fd=reader.as_raw_fd();
    let snapshots=Rc::new(RefCell::new(None::<SettingsSnapshot>));
    let wake_snapshots=Rc::clone(&snapshots);
    let notifications=ControlSignalClient::connect_session().ok().map(|signals| {
        let fd=signals.watch().fd;
        let queue=Rc::clone(&snapshots);
        (fd,Box::new(move || {
            if signals.on_ready(|signal| {
                if let ControlSignal::SettingsSnapshotChanged{snapshot}=signal { *queue.borrow_mut()=Some(snapshot);}
            }).is_ok(){UiWakeDrain::Keep}else{UiWakeDrain::Retire}
        }) as Box<dyn FnMut()->UiWakeDrain>)
    });
    run_with_controller_events_role_and_two_readable_fds(
        doc,
        UiWindowConfig{width:window_w,height:window_h,x:80,y:window_y,title:"FlameWM Explorer".into(),identity:Some(flamewm_ui_x11::X11WindowIdentity{
            // Matches flamewm-explorer.desktop (StartupWMClass) so the taskbar resolves the app icon.
            instance:"flamewm-explorer".to_owned(),class:"flamewm-explorer".to_owned(),
        })},
        UiWindowRole::Normal,fd,
        move||{
            let mut bytes=[0_u8;1024];
            loop {match reader.read(&mut bytes){Ok(0)=>return UiWakeDrain::Retire,Ok(_)=>{},Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>return UiWakeDrain::Keep,Err(_)=>return UiWakeDrain::Retire}}
        },notifications,
        move |event,document|{
            let mut app=event_app.borrow_mut();
            match event {
                UiControllerEvent::Action(action)=>{app.event(action);app.sync_scroll();},
                UiControllerEvent::Key(key)=>{app.key(key);app.sync_scroll();},
                // Settled client size: sizes, scroll window and rows all follow from `window` in project.
                UiControllerEvent::Resized{width,height}=>app.resize(*width,*height),
            }
            app.project(document)
        },
        move |document|{
            let mut app=wake_app.borrow_mut();
            if let Some(snapshot)=wake_snapshots.borrow_mut().take(){app.theme=ExplorerTheme::from_snapshot(&snapshot);}
            while let Ok(message)=results.try_recv(){app.wake(message);}
            app.sync_scroll();
            app.project(document)
        },
    )
}

/// Offset after a window-origin change: small steps accumulate, a folder change or a jump past the window snaps.
fn retarget_offset(offset:f32,step:f32,rows:usize)->f32 {
    if step.abs()>rows as f32 {0.0} else {offset+step}
}
/// One eased frame toward rest; values under 0.01 row settle to exactly zero.
fn ease_offset(offset:f32)->f32 {
    let next=offset*SCROLL_EASE;
    if next.abs()<0.01 {0.0} else {next}
}

#[cfg(test)]
mod internal_drag_tests {
    use super::*;
    #[test]
    fn drag_requires_threshold_and_release_can_cancel() {
        let mut drag=RowDrag::new(PathBuf::from("/tmp/item"),10.0,10.0);
        assert!(!drag.motion(13.0,14.0));
        assert!(!drag.dragging);
        assert!(drag.motion(18.0,10.0));
        assert!(drag.dragging);
        assert!(drag.motion(18.0,10.0));
    }
    #[test]
    fn tab_header_and_other_panes_cannot_be_silent_drop_targets() {
        let (tx,_rx)=mpsc::channel();
        let (scan_tx,_scan_rx)=mpsc::channel();
        let (chooser_tx,_chooser_rx)=mpsc::channel();
        let mut app=ExplorerApp::new(PathBuf::from("/tmp"),tx,scan_tx,chooser_tx);
        let dir=PathBuf::from("/tmp/folder");
        app.model.tab_mut().entries.push(Entry{
            path:dir.clone(),name:"folder".into(),directory:true,symlink:false,
            size:0,modified:None,
        });
        assert!(app.folder_at(150.0,80.0).is_none());
        assert!(app.folder_at(230.0,45.0).is_none());
        assert_eq!(app.folder_at(230.0,list_top(false)+1.0),Some(dir));
    }
}

#[cfg(test)]
mod window_frame_tests {
    use super::*;

    #[test]
    fn window_stays_above_taskbar_reserve() {
        for root in [(1350,641),(1600,900),(1920,1080),(800,600)] {
            let (w,h,y)=window_frame(root);
            // The framed window (client plus WM chrome) must end above the taskbar reserve.
            assert!(y>=0 && (y as u32)+h+FRAME_CHROME_HEIGHT<=root.1-TASKBAR_RESERVE, "root {root:?} window {w}x{h} at {y}");
            assert!(w<=root.0);
        }
        assert_eq!(window_frame((1920,1080)),(1250,740,65));
        assert_eq!(window_frame((1350,641)),(1215,493,59));
    }
}
#[cfg(test)]
mod glide_tests {
    use super::*;
    #[test]
    fn small_steps_glide_and_big_jumps_snap() {
        assert_eq!(retarget_offset(0.0,1.0,10),1.0);
        assert_eq!(retarget_offset(0.5,-1.0,10),-0.5);
        assert_eq!(retarget_offset(0.5,25.0,10),0.0);
    }
    #[test]
    fn ease_settles_within_about_150ms() {
        let mut offset=1.0_f32;
        let mut frames=0;
        while offset!=0.0 {offset=ease_offset(offset);frames+=1;assert!(frames<=12,"glide too long");}
        assert!(frames>=6,"glide too short to read as motion");
    }
}
