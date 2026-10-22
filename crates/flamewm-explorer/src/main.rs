//! FlameWM Explorer — native X11 retained UI; scans and transfers have separate workers.
mod theme;
mod mime;
use mime::{OpenWithChooser, CHOICE_ROWS};
use flamewm_explorer::ops::{self,Verdict};
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
use flamewm_explorer::thumbs::{self,Lookup,ThumbKey,Thumbnails,THUMB_SIZE};
use flamewm_explorer::columns::{Divider,Widths};
use flamewm_explorer::scroll::{self,Resize,ResizeGate,RowGlide};
use flamewm_explorer::{archive,properties,refresh::DirWatch,search::{SearchEvent,SearchWorker},trash,places as bookmarks};
use std::sync::atomic::{AtomicBool,Ordering};
use std::sync::Arc;
use flamewm_explorer::jobs::{self,CancellationToken,Conflict,Undo};
use flamewm_explorer::remote::{self,CopyEvent};
use flamewm_remote_fs::{Cancel as RemoteCancel,Config as RemoteConfig,Error as RemoteError,HostKeyPrompt,ListEvent,RemoteFs,TransferEvent,Url as RemoteUrl};
use theme::ExplorerTheme;
use flamewm_control_core::{ControlRequest,ControlResponse};
use flamewm_control_dbus::{ControlClient,ControlSignal,ControlSignalClient};
use flamewm_dbus_reactor::BusKind;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_desktop_core::{file_actions::create_new_folder,trash::Trash};
use flamewm_ui_x11::{UiActionEvent,UiActionPhase,UiColor,UiControllerEvent,UiDocumentAccess,UiDragEvent,UiKeyEvent,UiKeyPhase,UiWakeDrain,UiWindowConfig,UiWindowRole,run_with_controller_events_role_and_two_readable_fds,decode_document};

const UI: &[u8]=include_bytes!(concat!(env!("OUT_DIR"),"/flamewm-explorer.rwr"));
/// Menu commands that also work in remote folders; every other command is refused there until its remote path exists.
const REMOTE_ACTIONS: [&str;7]=["open","showtype","hidden","copy","paste","rename","newfolder"];
const MENU_ITEMS: [&str;25]=["connect","showtype","open","openwith","cut","copy","paste","rename","duplicate","newfolder","bookmark","trash","properties","terminal","compress","extract","newtab","opencontaining","restore","emptytrash","placerename","placeremove","unmount","eject","hidden"];
/// Sidebar rows (group headings plus places) that the template provides; extra rows are not drawn.
const SIDEBAR_SLOTS:usize=32;
// Canonical geometry: paint (project) and hit-testing (cell_rect, folder_at, select bands) all derive from these.
const TOOLBAR_HEIGHT:f32=45.0;
const TABBAR_HEIGHT:f32=36.0;
const HEADER_HEIGHT:f32=29.0;
/// Half the hit width of a column divider grip, centred on the divider line.
const COLUMN_GRIP_HALF:f32=4.0;
/// Scrollbar strip on the list's right edge; the thumb is inset inside it.
const SCROLLBAR_TRACK_WIDTH:f32=10.0;
const SCROLLBAR_THUMB_WIDTH:f32=6.0;
const SCROLLBAR_THUMB_INSET:f32=2.0;
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
/// Frame interval of the sub-row scroll glide (timing lives in `scroll::RowGlide`).
const SCROLL_FRAME:Duration=Duration::from_millis(16);
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
    /// Work on a remote folder. Blocking ssh calls run here, never on the UI thread.
    Remote {fs:RemoteFs,op:RemoteOp},
}
enum RemoteOp {
    NewFolder {parent:RemoteUrl},
    Rename {from:RemoteUrl,to:RemoteUrl},
    Delete {urls:Vec<RemoteUrl>},
    Copy {sources:Vec<PathBuf>,directory:PathBuf,cancel:CancellationToken},
}
enum ChooserJob {
    Discover { request: u64, path: PathBuf },
    SetDefault { mime: String, desktop_id: String },
}
fn chooser_worker(rx: Receiver<ChooserJob>, result: Sender<WorkerResult>, mut wake: UnixStream) {
    for job in rx {
        match job {
            ChooserJob::Discover { request, path } => {
                let chooser = mime::discover(path, &packaged_root());
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
    /// Default-app lookup for a double-clicked file; `Ok(false)` means no default, so the chooser opens.
    OpenResolved{path:PathBuf,result:Result<bool,String>},
    /// Decoded 24px row thumbnail (`None` when the image could not be thumbnailed).
    Thumbnail{key:ThumbKey,pixels:Option<Vec<u8>>},
    /// Trailing-edge timer for held resize sizes.
    ResizeFlush,
    /// Streamed remote listing chunk, converted off the UI thread.
    RemoteRows {generation:u64,path:PathBuf,rows:Vec<Entry>},
    /// End of a remote listing; `failure` is worded for the status strip.
    RemoteDone {generation:u64,path:PathBuf,failure:Option<String>},
    /// Remote file downloaded to the cache (`Ok`) or failed (`Err`).
    RemoteDownloaded(Result<PathBuf,String>),
    /// The listing needs the user first: an unknown host key or a failed sign-in.
    RemoteBlocked {generation:u64,path:PathBuf,error:RemoteError},
    /// Host key fetched for the trust dialog; `Err` is worded for the status strip.
    HostKeyProbed {url:RemoteUrl,result:Result<HostKeyPrompt,String>},
    /// Outcome of appending the confirmed host key to known_hosts.
    HostKeyTrusted {url:RemoteUrl,result:Result<(),String>},
}
/// Runs one remote operation on the file worker and reports it through the same result paths as local work.
fn remote_job(fs:&RemoteFs,op:RemoteOp,wake:&mut UnixStream,result:&Sender<WorkerResult>) {
    match op {
        RemoteOp::NewFolder{parent}=>{
            let done=remote::new_folder(fs,&parent).map(|name|format!("Created {name}")).map_err(|e|remote::describe(&e));
            send_wake(wake,result,WorkerResult::Completed(done));
        },
        RemoteOp::Rename{from,to}=>{
            let done=remote::rename_item(fs,&from,&to).map(|()|"Renamed".to_owned()).map_err(|e|remote::describe(&e));
            send_wake(wake,result,WorkerResult::Completed(done));
        },
        RemoteOp::Delete{urls}=>{
            let report=remote::delete_items(fs,&urls);
            send_wake(wake,result,WorkerResult::BatchCompleted{report,action:"Deleted",cut_sources:None});
        },
        RemoteOp::Copy{sources,directory,cancel}=>{
            let mut last_progress=Instant::now();
            let report=remote::copy_items(fs,&sources,&directory,&cancel,&mut |event|match event {
                CopyEvent::Total(bytes)=>send_wake(wake,result,WorkerResult::Total(bytes)),
                CopyEvent::Progress(bytes)=>if last_progress.elapsed()>=Duration::from_millis(100) {
                    send_wake(wake,result,WorkerResult::Progress(bytes));
                    last_progress=Instant::now();
                },
            });
            send_wake(wake,result,WorkerResult::BatchCompleted{report,action:"Copied",cut_sources:None});
        },
    }
}
/// Decodes queued image keys one at a time; the UI reads results from the cache, never waits on this thread.
fn thumb_worker(rx:Receiver<ThumbKey>, result:Sender<WorkerResult>, mut wake:UnixStream) {
    for key in rx {
        let pixels=thumbs::render_thumbnail(&key);
        send_wake(&mut wake,&result,WorkerResult::Thumbnail{key,pixels});
    }
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
            WorkerJob::Remote{fs,op}=>remote_job(&fs,op,&mut wake,&result),
        }
    }
}
/// Work that waits for a dialog answer before it touches files.
enum Pending {
    Delete(Vec<PathBuf>),
    Trash(Vec<PathBuf>),
    EmptyTrash(PathBuf),
    Transfer {sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,conflict:Conflict},
    RemoteDelete(Vec<PathBuf>),
    Drop{sources:Vec<PathBuf>,directory:PathBuf,force_copy:bool},
    RemoteCopy{sources:Vec<PathBuf>,directory:PathBuf},
}
/// One modal at a time. Buttons map to fixed slots: primary, secondary, tertiary, cancel.
enum Dialog {
    Properties {props:properties::Properties, size:Option<u64>, cancel:Arc<AtomicBool>},
    Conflicts {sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,count:usize},
    Confirm {title:String, lines:Vec<String>, pending:Pending},
    /// Unknown host key: `prompt` is `None` while the key is being fetched.
    HostKey {url:RemoteUrl, prompt:Option<HostKeyPrompt>},
    AuthFailed {url:RemoteUrl, password_possible:bool},
}
#[derive(Clone,Copy)]
enum EditMode { Path, Search, Rename, PlaceName, Mode }
struct ExplorerApp {
    model:Explorer, tx:Sender<WorkerJob>, scan_tx:Sender<WorkerJob>,
    chooser_tx:Sender<ChooserJob>, chooser:Option<OpenWithChooser>,
    // Row thumbnails: the cache and the image id each row slot currently shows. `project` takes `&self`, hence the cells.
    thumbs:RefCell<Thumbnails>, thumb_tx:Option<Sender<ThumbKey>>, thumb_bound:RefCell<[Option<u64>;ROW_SLOTS]>,
    chooser_loading:bool, chooser_request:u64, places:Vec<Place>,
    context:Option<(f32,f32)>, edit:Option<EditMode>, field:flamewm_explorer::TextField, rename_target:Option<PathBuf>, theme:ExplorerTheme, window:(f32,f32),
    last_click:Option<(PathBuf,Instant)>, progress:Option<u64>,
    context_on_entry:bool, rubber:Option<((f32,f32),(f32,f32))>, activity:Option<String>, busy:bool,
    row_drag:Option<RowDrag>, drop_folder:Option<PathBuf>, file_drag_out:Option<Vec<PathBuf>>,
    /// Ark drag-out extractions in flight; polled from the event loop.
    ark_extracts: Vec<std::sync::mpsc::Receiver<Result<flamewm_explorer::dndext::ExtractReport, flamewm_explorer::dndext::ExtractError>>>, cancel:CancellationToken,
    menu_overflow:bool, glide:RowGlide, scroll_origin:usize, scroll_ticking:bool,
    /// The window origin or its end-of-glide position changed since the last full projection.
    list_dirty:bool,
    /// Resize coalescing (one relayout per interval) and whether its trailing-edge timer is already queued.
    resize_gate:ResizeGate, resize_flush_scheduled:bool,
    /// Header columns: fixed widths for Modified, Size and Type, and whether the Type column is shown.
    widths:Widths, show_type:bool, col_drag:Option<(Divider,f32)>,
    /// Scrollbar thumb drag: pointer y and the origin at press time.
    scroll_drag:Option<(f32,usize)>,
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
    /// One remote client per process; listings and downloads run on its workers.
    remote:RemoteFs,
    /// Cancels the streaming listing that is still running; replaced on the next listing.
    remote_cancel:Option<RemoteCancel>,
    /// Recently visited remote URLs (no secrets), newest first, persisted in explorer.toml.
    remote_recent:Vec<String>,
}
impl ExplorerApp {
    fn new(initial:PathBuf,tx:Sender<WorkerJob>,scan_tx:Sender<WorkerJob>,chooser_tx:Sender<ChooserJob>) -> Self {
        let home=home_dir();
        let trash_base=std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".local/share")).join("Trash");
        let mut app=Self {model:Explorer::new(initial),tx,scan_tx,chooser_tx,chooser:None,thumbs:RefCell::new(Thumbnails::default()),thumb_tx:None,thumb_bound:RefCell::new([None;ROW_SLOTS]),chooser_loading:false,chooser_request:0,places:Vec::new(),context:None,edit:None,field:flamewm_explorer::TextField::default(),rename_target:None,theme:ExplorerTheme::default(),window:(WINDOW_WIDTH as f32,WINDOW_HEIGHT as f32),last_click:None,progress:None,context_on_entry:false,rubber:None,activity:None,busy:false,row_drag:None,drop_folder:None,file_drag_out:None,
            ark_extracts:Vec::new(),cancel:CancellationToken::new(),menu_overflow:false,glide:RowGlide::default(),scroll_origin:0,scroll_ticking:false,list_dirty:false,resize_gate:ResizeGate::default(),resize_flush_scheduled:false,widths:Widths::default(),show_type:false,col_drag:None,scroll_drag:None,column_sort:ColumnSort::default(),typed:String::new(),typed_at:None,
            dialog:None,ark:archive::find_ark().is_some(),search:None,search_id:0,search_text:String::new(),search_rows:None,watch:None,watched:None,context_place:None,progress_total:0,trash_base,unmounted:Vec::new(),events:None,system_spent:None,remote:RemoteFs::new(RemoteConfig::default()),remote_cancel:None,remote_recent:prefs::load_recent(&prefs::config_dir())};
        app.reload_places();
        app
    }
    /// Starts a text edit. `select` marks a char range that the next typed character replaces.
    fn begin_edit(&mut self,mode:EditMode,text:String,select:Option<(usize,usize)>) {
        self.edit=Some(mode);
        match select {
            Some(range)=>self.field.set_selected(text,range),
            None=>self.field.set(text),
        }
    }
    /// Applies a finished text edit: address, search, permissions, place name or file rename.
    fn commit_edit(&mut self,edit:EditMode,content:&str) {
        match edit {
            EditMode::Path=>match parse_address(content,&home_dir()) {
                Ok(AddressTarget::Local(path))=>self.navigate(path),
                Ok(AddressTarget::Remote(url))=>self.navigate(remote::to_path(&url)),
                Ok(AddressTarget::ExternalUri(uri))=>{
                    match Command::new("xdg-open").arg(&uri).spawn() {
                        Ok(_)=>self.model.status="Remote URL passed to the installed system handler".into(),
                        Err(e)=>self.model.status=format!("Remote handler unavailable: {e}"),
                    }
                },
                Err(reason)=>self.model.status=reason,
            },
            EditMode::Search=>self.start_search(content.to_owned()),
            EditMode::Mode=>self.apply_mode(content),
            EditMode::PlaceName=>{
                self.context_place=None;
                if let Some(path)=self.rename_target.take() {self.mutate_bookmark(&path,|items,index|bookmarks::rename(items,index,content));}
            },
            EditMode::Rename=>{
                if let Some(path)=self.rename_target.take() {self.commit_rename(path,content);}
            },
        }
    }
    /// Validates the typed name, then renames `path` through the remote client or the local file worker.
    fn commit_rename(&mut self,path:PathBuf,content:&str) {
        match flamewm_desktop_core::file_actions::validate_rename_name(content) {
            Ok(name)=>{
                if path.file_name().map(|n|n.to_string_lossy().into_owned())==Some(name.to_string()) {return;}
                if self.busy {self.model.status="File operation already in progress".into();return;}
                if let Some(from)=remote::url_of(&path) {
                    match from.parent() {
                        Some(parent)=>{let to=parent.join(&name);self.send_remote(RemoteOp::Rename{from,to},"Renaming…");},
                        None=>self.model.status="Cannot rename this folder".into(),
                    }
                } else {
                    let to=path.with_file_name(name);
                    if self.tx.send(WorkerJob::Rename{from:path,to}).is_ok() {
                        self.busy=true;self.model.status="Renaming…".into();
                    }else{self.model.status="File worker unavailable".into();}
                }
            },
            Err(e)=>self.model.status=e.to_string(),
        }
    }
    /// The single selected folder entry, when the context menu is on an entry.
    fn selected_folder(&self)->Option<PathBuf> {
        if self.model.tab().selected_paths.len()!=1 {return None;}
        let path=self.selected()?;
        self.model.tab().entries.iter().any(|entry|entry.path==path && entry.directory).then_some(path)
    }
    /// Folder that Open Terminal Here and Add Folder to Places act on: a right-clicked folder, else the current folder.
    fn target_directory(&self)->PathBuf {
        if self.context_on_entry {if let Some(folder)=self.selected_folder() {return folder;}}
        self.model.tab().path().to_path_buf()
    }
    /// Clamps a rubber-band point into the file list: right of the sidebar, below the header, above the status strip.
    fn clamp_to_list(&self,x:f32,y:f32)->(f32,f32) {
        let top=list_top(self.model.tabs.len()>1);
        let bottom=(self.window.1-STATUS_STRIP_HEIGHT).max(top);
        (x.clamp(SIDEBAR_WIDTH,self.window.0.max(SIDEBAR_WIDTH)),y.clamp(top,bottom))
    }
    /// Folder drops (row drags and external drops) confirm first, with the same validator and dialog as paste.
    fn confirm_drop(&mut self,sources:Vec<PathBuf>,directory:PathBuf,force_copy:bool) {
        if self.busy {self.model.status="File operation already in progress".into();return;}
        let writable=fs::metadata(&directory).is_ok_and(|m|m.is_dir() && !m.permissions().readonly());
        let kind=if force_copy {ops::Kind::Copy} else {ops::Kind::Move};
        let sources=match ops::validate(kind,&source_facts(&sources),Some(&directory),writable) {
            Verdict::Run(sources)=>sources,
            Verdict::NoOp=>{self.model.status=(if force_copy {"Drop cancelled (nothing to copy)"} else {"Drop cancelled (nothing to move)"}).into();return;},
            Verdict::Refuse(message)=>{self.model.status=message.into();return;},
        };
        let title=match (force_copy,sources.len()) {(true,1)=>"Copy file?",(true,_)=>"Copy items?",(false,1)=>"Move file?",(false,_)=>"Move items?"};
        let location=directory.clone();
        self.confirm_items(title,&sources,&location,Pending::Drop{sources:sources.clone(),directory,force_copy});
    }
    /// Remote copies from drags or drops confirm first; the remote worker copies on OK.
    fn confirm_remote_copy(&mut self,sources:Vec<PathBuf>,directory:PathBuf) {
        if sources.is_empty() {self.model.status="Nothing to copy".into();return;}
        let name=match sources.as_slice() {
            [only]=>only.file_name().unwrap_or(only.as_os_str()).to_string_lossy().into_owned(),
            many=>format!("{} items",many.len()),
        };
        let location=remote::url_of(&directory).map(|url|remote::display_text(&url)).unwrap_or_else(||ops::display_location(&directory,&home_dir()));
        let title=if sources.len()==1 {"Copy file?"} else {"Copy items?"};
        self.dialog=Some(Dialog::Confirm{title:title.into(),lines:vec![name,location],pending:Pending::RemoteCopy{sources,directory}});
    }
    /// Built-in and bookmark places, plus the Trash folder just before mounted devices.
    fn reload_places(&mut self) {
        let home=home_dir();
        let mut list=places(&home,&self.unmounted);
        // Trash sits at the end of the Places group, as in the mock.
        let trash=Place{name:"Trash".into(),path:self.trash_base.join("files"),removable:false,group:PlaceGroup::Places,icon:"trash",device:None};
        let at=list.iter().rposition(|p|p.group==PlaceGroup::Places).map_or(0,|i|i+1);
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
        if remote::is_remote(self.model.tab().path()) {
            self.model.status="Search is not available in remote folders yet".into();
            return;
        }
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
    /// Settled client size from the WM. Returns true when the layout must be projected now. Sizes that
    /// arrive within `RESIZE_INTERVAL` of the last applied one are held; the trailing timer applies the last.
    fn resize(&mut self,width:u32,height:u32)->bool {
        let size=(width.max(1),height.max(1));
        match self.resize_gate.offer(size,Instant::now()) {
            Resize::Apply=>{self.apply_size(size);true},
            Resize::Defer(delay)=>{
                if !self.resize_flush_scheduled {
                    self.resize_flush_scheduled=true;
                    self.spawn_background(move||{std::thread::sleep(delay);WorkerResult::ResizeFlush});
                }
                false
            },
        }
    }
    fn flush_resize(&mut self) {
        self.resize_flush_scheduled=false;
        if let Some(size)=self.resize_gate.flush(Instant::now()) {self.apply_size(size);}
    }
    fn apply_size(&mut self,(width,height):(u32,u32)) {
        self.window=(width as f32,height as f32);
        self.sync_scroll();
    }
    fn sync_scroll(&mut self) {
        let rows=self.visible_rows();
        let origin=self.origin(rows);
        if origin!=self.scroll_origin {
            let step=origin as isize-self.scroll_origin as isize;
            self.glide.origin_moved(step,Instant::now(),rows);
            self.scroll_origin=origin;
            self.list_dirty=true;
        }
        if self.glide.is_animating(Instant::now()) {self.schedule_scroll_tick();}
    }
    fn schedule_scroll_tick(&mut self) {
        if self.scroll_ticking {return;}
        self.scroll_ticking=true;
        self.spawn_background(||{std::thread::sleep(SCROLL_FRAME);WorkerResult::ScrollTick});
    }
    /// One frame of the glide. The frame that finds the glide finished marks the list dirty so its rest position is projected.
    fn step_scroll(&mut self) {
        self.scroll_ticking=false;
        if self.glide.is_animating(Instant::now()) {self.schedule_scroll_tick();} else {self.list_dirty=true;}
    }
    /// Current preferences, as persisted. Every setting writes the whole file so none can drop another.
    fn current_prefs(&self)->prefs::Prefs {
        prefs::Prefs{show_hidden:self.model.hidden,show_type:self.show_type,col_modified:self.widths.modified as u32,col_size:self.widths.size as u32,col_kind:self.widths.kind as u32}
    }
    fn save_prefs(&mut self) {
        if let Err(error)=prefs::save(&prefs::config_dir(),&self.current_prefs()) {
            self.model.status=format!("Could not save Explorer settings: {error}");
        }
    }
    /// Show or hide dot files, persisting the choice for the next Explorer window.
    fn toggle_hidden(&mut self) {
        self.model.hidden=!self.model.hidden;
        self.save_prefs();
        self.scan();
    }
    /// Shows or hides the Type column (menu option), persisting the choice.
    fn toggle_show_type(&mut self) {
        self.show_type=!self.show_type;
        self.save_prefs();
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
        if let Some(previous)=self.remote_cancel.take() {previous.cancel();}
        if let Some(url)=remote::url_of(&path) {
            self.watch=None;
            self.watched=None;
            self.start_remote_listing(generation,url);
            return;
        }
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
    /// Streams a remote folder: the worker converts each chunk and the UI loop appends it. The UI never waits on ssh.
    fn start_remote_listing(&mut self,generation:u64,url:RemoteUrl) {
        let cancel=RemoteCancel::new();
        self.remote_cancel=Some(cancel.clone());
        self.model.tab_mut().entries.clear();
        self.model.status="Loading…".into();
        let path=remote::to_path(&url);
        let events=self.remote.list_dir(&url,&cancel);
        let Some((tx,wake))=self.events.as_ref().and_then(|(tx,wake)|Some((tx.clone(),wake.try_clone().ok()?))) else {
            self.model.status="Remote listing needs the event loop".into();
            return;
        };
        std::thread::spawn(move||forward_listing(generation,path,url,events,tx,wake));
    }
    fn save_remote_recent(&mut self) {
        if let Err(error)=prefs::save_recent(&prefs::config_dir(),&self.remote_recent) {
            self.model.status=format!("Recent servers not saved: {error}");
        }
    }
    /// Downloads a remote file into the cache, then opens the local copy with the default application.
    fn download_and_open(&mut self,url:RemoteUrl) {
        let Some((tx,wake))=self.events.as_ref().and_then(|(tx,wake)|Some((tx.clone(),wake.try_clone().ok()?))) else {
            self.model.status="Remote open needs the event loop".into();
            return;
        };
        let local=remote::cache_path(&remote::cache_root(),&url,url.file_name());
        if let Some(parent)=local.parent() {
            if let Err(error)=fs::create_dir_all(parent) {
                self.model.status=format!("Cannot create download cache: {error}");
                return;
            }
        }
        let transfer=self.remote.download(&url,&local,&RemoteCancel::new());
        self.model.status=format!("Downloading {}…",url.file_name());
        std::thread::spawn(move||forward_download(local,transfer,tx,wake));
    }
    fn navigate(&mut self,path:PathBuf) {
        if remote::is_remote(&path) {
            self.clear_search();
            self.model.tab_mut().visit(path);
            self.scan();
            self.context=None;
            return;
        }
        // Opening Trash creates the XDG layout (files/ and info/) when it is missing; it never errors on a fresh home.
        if path==self.trash_base.join("files") {
            if let Err(e)=fs::create_dir_all(&path).and_then(|()|fs::create_dir_all(self.trash_base.join("info"))) {
                self.model.status=format!("Cannot create Trash: {e}");
                return;
            }
        }
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
    /// The files a row drag carries: the whole selection when the pressed row is part of it.
    fn dragged_sources(&self,source:&Path)->Vec<PathBuf> {
        if self.model.tab().selected_paths.contains(source) {self.selected_files()} else {vec![source.to_path_buf()]}
    }
    /// Files dropped from another application copy into the current folder. XDND only offers copy here.
    fn ark_drop(&mut self,service:String,path:String) {
        if remote::is_remote(self.model.tab().path()) {
            self.model.status="Drops into remote folders are not available yet".into();
            return;
        }
        let destination=self.model.tab().path().to_path_buf();
        self.ark_extracts.push(flamewm_explorer::dndext::spawn_ark_extract(service,path,destination));
        self.model.status="Extracting from Ark…".into();
    }
    /// External drop: a folder row under the pointer receives the files; anywhere else means the current folder.
    fn external_drop(&mut self,paths:Vec<PathBuf>,x:f32,y:f32) {
        let destination=self.folder_at(x,y).unwrap_or_else(||self.model.tab().path().to_path_buf());
        if remote::is_remote(&destination) {self.confirm_remote_copy(paths,destination);}
        else {self.confirm_drop(paths,destination,true);}
    }
    fn finish_row_drag(&mut self,e:&UiActionEvent)->bool {
        let Some(drag)=self.row_drag.take() else {return false;};
        self.drop_folder=None;
        if !drag.dragging {return false;}
        if remote::is_remote(&drag.source) || remote::is_remote(self.model.tab().path()) || self.folder_at(e.x,e.y).is_some_and(|folder|remote::is_remote(&folder)) {
            // Remote drags copy (no moves); the drop folder may be local or remote.
            let Some(destination)=self.folder_at(e.x,e.y) else {
                self.model.status="Drop cancelled (choose a folder)".into();
                return true;
            };
            let sources:Vec<PathBuf>=self.dragged_sources(&drag.source).into_iter().filter(|s|s.parent()!=Some(destination.as_path())).collect();
            self.last_click=None;
            self.context=None;
            self.confirm_remote_copy(sources,destination);
            return true;
        }
        self.last_click=None;
        self.context=None;
        let Some(destination)=self.folder_at(e.x,e.y) else {
            self.model.status="Drop cancelled (choose a folder)".into();
            return true;
        };
        let sources=self.dragged_sources(&drag.source);
        // Control modifies the operation at release time, exactly like file managers.
        self.confirm_drop(sources,destination,e.ctrl_down());
        true
    }
    /// Unknown host key: fetch the key for the trust dialog. Failed sign-in: retry dialog. Others: status only.
    fn remote_blocked(&mut self,path:PathBuf,error:RemoteError) {
        let Some(url)=remote::url_of(&path) else {return};
        match &error {
            RemoteError::HostKeyUnknown{..}=>{
                self.model.status="Checking the server's host key…".into();
                self.dialog=Some(Dialog::HostKey{url:url.clone(),prompt:None});
                let Some((tx,mut wake))=self.events.as_ref().and_then(|(tx,wake)|Some((tx.clone(),wake.try_clone().ok()?))) else {
                    self.dialog=None;
                    self.model.status="Host key check needs the event loop".into();
                    return;
                };
                let fs=self.remote.clone();
                std::thread::spawn(move||{
                    let result=fs.probe_host_key(&url).wait().map_err(|e|remote::describe(&e));
                    if tx.send(WorkerResult::HostKeyProbed{url,result}).is_ok() {let _=wake.write_all(&[1]);}
                });
            },
            RemoteError::AuthFailed{password_possible,..}=>{
                self.model.status=remote::describe(&error);
                self.dialog=Some(Dialog::AuthFailed{url,password_possible:*password_possible});
            },
            other=>self.model.status=remote::describe(other),
        }
    }
    /// Appends the user-confirmed key to known_hosts on a thread (the file write and the check never run on the UI thread).
    fn save_host_key(&mut self,url:RemoteUrl,prompt:HostKeyPrompt) {
        let Some((tx,mut wake))=self.events.as_ref().and_then(|(tx,wake)|Some((tx.clone(),wake.try_clone().ok()?))) else {
            self.model.status="Host key save needs the event loop".into();
            return;
        };
        self.model.status="Saving host key…".into();
        let fs=self.remote.clone();
        std::thread::spawn(move||{
            let result=fs.trust_host_key(&prompt).wait().map_err(|e|remote::describe(&e));
            if tx.send(WorkerResult::HostKeyTrusted{url,result}).is_ok() {let _=wake.write_all(&[1]);}
        });
    }
    fn open(&mut self, path:&Path) {
        if let Some(url)=remote::url_of(path) {
            if self.model.tab().entries.iter().any(|entry|entry.path==path && entry.directory) {self.navigate(path.to_path_buf());}
            else {self.download_and_open(url);}
            return;
        }
        if path.is_dir() {self.navigate(path.to_path_buf());return;}
        // A .desktop file is not executed as a shell script. XDG application handlers own launch policy.
        if is_archive(path) {
            match Command::new("ark").arg(path).spawn() {
                Ok(_)=>return,
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
                Err(e)=>{self.model.status=format!("Ark failed: {e}");return;},
            }
        }
        // The XDG default application opens the file; with none configured, the user chooses (no browser fallback).
        // xdg-mime and the catalog run on a worker; the result returns through the event loop.
        let path=path.to_path_buf();
        let Some((tx,mut wake))=self.events.as_ref().and_then(|(tx,wake)|Some((tx.clone(),wake.try_clone().ok()?))) else {
            match mime::open_with_default(&path) {
                Ok(true)=>{},
                Ok(false)=>self.request_open_with(path),
                Err(e)=>self.model.status=format!("Open failed: {e}"),
            }
            return;
        };
        self.model.status="Opening…".into();
        std::thread::spawn(move||{
            let result=mime::open_with_default(&path);
            if tx.send(WorkerResult::OpenResolved{path,result}).is_ok() {let _=wake.write_all(&[1]);}
        });
    }
    fn start_transfer(&mut self,sources:Vec<PathBuf>,move_it:bool) {
        let directory=self.model.tab().path().to_path_buf();
        self.start_transfer_to(sources,directory,move_it);
    }
    /// Duplicate: copy each source beside itself as `name (copy)`; no conflict prompt, since the name is always fresh.
    fn start_duplicate(&mut self,sources:Vec<PathBuf>) {
        if self.busy {self.model.status="File operation already in progress".into();return;}
        if sources.is_empty() {self.model.status="Select files first".into();return;}
        if sources.iter().any(|s|remote::is_remote(s)) {self.model.status="Duplicate is not available in remote folders yet".into();return;}
        let directory=self.model.tab().path().to_path_buf();
        let writable=fs::metadata(&directory).is_ok_and(|m|m.is_dir() && !m.permissions().readonly());
        match ops::validate(ops::Kind::Copy,&source_facts(&sources),Some(&directory),writable) {
            Verdict::Run(sources)=>self.send_transfer(sources,directory,false,Conflict::Duplicate),
            Verdict::NoOp=>{},
            Verdict::Refuse(message)=>self.model.status=message.into(),
        }
    }
    fn start_transfer_to(&mut self,sources:Vec<PathBuf>,directory:PathBuf,move_it:bool) {
        if self.busy {self.model.status="File operation already in progress".into();return;}
        if sources.is_empty() {self.model.status="Select files first".into();return;}
        // Local-to-remote and remote-to-local transfers copy through the remote worker; they never reach std::fs.
        if remote::is_remote(&directory) || sources.iter().any(|s|remote::is_remote(s)) {
            if move_it {self.model.status="Moving to or from remote folders is not available yet; copy instead".into();return;}
            self.start_remote_copy(sources,directory);
            return;
        }
        let writable=fs::metadata(&directory).is_ok_and(|m|m.is_dir() && !m.permissions().readonly());
        let kind=if move_it {ops::Kind::Move} else {ops::Kind::Copy};
        // Redundant transfers (move into its own folder, missing sources) are dropped without a popup.
        let sources=match ops::validate(kind,&source_facts(&sources),Some(&directory),writable) {
            Verdict::Run(sources)=>sources,
            Verdict::NoOp=>return,
            Verdict::Refuse(message)=>{self.model.status=message.into();return;},
        };
        // Existing names need one answer for the whole batch; the default keeps both.
        let count=sources.iter().filter(|s|s.file_name().is_some_and(|n|fs::symlink_metadata(directory.join(n)).is_ok())).count();
        if count>0 {self.dialog=Some(Dialog::Conflicts{sources,directory,move_it,count});return;}
        if move_it {
            let title=if sources.len()==1 {"Move file?"} else {"Move items?"};
            self.confirm_items(title,&sources,&directory,Pending::Transfer{sources:sources.clone(),directory:directory.clone(),move_it,conflict:Conflict::KeepBoth});
            return;
        }
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
    /// Queues a remote operation on the file worker. Shares the busy flag with local file work.
    fn send_remote(&mut self,op:RemoteOp,status:&str)->bool {
        if self.busy {self.model.status="File operation already in progress".into();return false;}
        if self.tx.send(WorkerJob::Remote{fs:self.remote.clone(),op}).is_ok() {
            self.busy=true;
            self.model.status=status.into();
            true
        } else {
            self.model.status="File worker unavailable".into();
            false
        }
    }
    /// Copies local and remote files into `directory` (a local or remote folder). Copy only: no moves.
    fn start_remote_copy(&mut self,sources:Vec<PathBuf>,directory:PathBuf) {
        if sources.is_empty() {self.model.status="Nothing to copy".into();return;}
        let count=sources.len();
        let op=RemoteOp::Copy{sources,directory,cancel:self.cancel.clone()};
        if self.send_remote(op,&format!("Copying {count} item(s)…")) {
            self.progress=Some(0);
            self.activity=None;
        }
    }
    fn tab_shows(&self,url:&RemoteUrl)->bool {
        remote::url_of(self.model.tab().path()).is_some_and(|tab|tab.to_string()==url.to_string())
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
    /// Opens the configured terminal (FlameWM Terminal by default) in `directory`. Only FlameWM Terminal takes
    /// `--working-directory`; other emulators get the directory as their working directory and no command.
    fn spawn_terminal(&mut self,directory:PathBuf) {
        let program=flamewm_applications::terminal::terminal_program();
        let mut argv=flamewm_applications::terminal::terminal_command(&program,&[],Some(&directory));
        if argv.len()>1 && matches!(argv.last().map(String::as_str),Some("-e"|"--")) {argv.pop();}
        self.model.status=match Command::new(&argv[0]).args(&argv[1..]).current_dir(&directory).spawn() {
            Ok(_)=>"Opened terminal".into(),
            Err(e)=>format!("Terminal failed: {e}"),
        };
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
        // Ark requires either a filename or --autofilename; without it the dialog only shows an error.
        match Command::new("ark").args(["--add","--autofilename","zip"]).args(&paths).spawn() {
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
                // Copying into the folder the items already live in is a duplicate: a fresh `name (copy)`, no prompt.
                let here=self.model.tab().path().to_path_buf();
                if !cut && sources.iter().all(|s|s.parent()==Some(here.as_path())) {self.start_duplicate(sources);}
                else {self.start_transfer(sources,cut);}
            },
            None=>self.model.status="Nothing to paste".into(),
        }
    }
    fn action_menu(&mut self,cmd:&str) {
        if remote::is_remote(self.model.tab().path()) && !REMOTE_ACTIONS.contains(&cmd) {
            self.model.status="Not available in remote folders yet".into();
            self.context=None;
            return;
        }
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
            "open"=>if let Some(index)=self.context_place {self.activate_place(index)} else if let Some(path)=selected {self.open(&path)},
            "openwith"=>if let Some(path)=selected {self.request_open_with(path)},
            "copy"|"cut"=>{
                if !paths.is_empty() {
                    self.model.status=format!("{} {} item(s)",if cmd=="cut"{"Cut"}else{"Copied"},paths.len());
                    self.model.clipboard=Some((paths.clone(),cmd=="cut"));
                    self.publish_system_clipboard(paths,cmd=="cut");
                }
            },
            "paste"=>self.request_paste(),
            "duplicate"=>self.start_duplicate(paths),
            "newfolder"=>{
                if let Some(parent)=remote::url_of(self.model.tab().path()) {
                    self.send_remote(RemoteOp::NewFolder{parent},"Creating folder…");
                } else {
                    let directory=self.model.tab().path().to_path_buf();
                    if self.tx.send(WorkerJob::Create{directory}).is_ok() {
                        self.busy=true;self.model.status="Creating folder…".into();
                    }else{self.model.status="File worker unavailable".into();}
                }
            },
            "rename"=>if let Some(path)=selected {
                // Inline rename: the name cell becomes the field, with the stem selected (folders select all).
                let name=path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let folder=self.model.tab().entries.iter().any(|entry|entry.path==path && entry.directory);
                let select=flamewm_explorer::rename_selection(&name,folder);
                self.rename_target=Some(path);
                self.begin_edit(EditMode::Rename,name,Some(select));
            },
            "trash"=>self.request_trash(paths),
            "terminal"=>{let directory=self.target_directory();self.spawn_terminal(directory);},
            "compress"=>self.compress_selected(),
            "extract"=>if let Some(path)=selected.as_deref() {
                // Ark opens a destination dialog; no implicit overwrites in Explorer.
                self.spawn_ark(&["--batch","--dialog"],path);
            },
            "newtab"=>if self.model.tabs.len()<5 {
                // From the sidebar menu the new tab opens that place; from a file or background menu it repeats the folder.
                let target=self.context_place_path().or_else(||if self.context_on_entry {self.selected_folder()} else {None});
                self.model.new_tab();self.scan();
                if let Some(path)=target {self.navigate(path);}
            },
            "bookmark"=>{let path=self.target_directory();match add_bookmark(&path,&home_dir()) {Ok(())=>{self.reload_places();self.model.status="Added to Places".into();},Err(e)=>self.model.status=format!("Bookmark failed: {e}")}},
            "properties"=>if let Some(path)=self.context_place_path().or(selected) {self.open_properties(path)},
            "opencontaining"=>if let Some(path)=selected {self.open_containing(path)},
            "restore"=>if let Some(path)=selected {
                if self.tx.send(WorkerJob::Restore{base:self.trash_base.clone(),stored:path}).is_ok() {
                    self.busy=true;self.model.status="Restoring…".into();
                }
            },
            "emptytrash"=>self.dialog=Some(Dialog::Confirm{
                title:"Empty Trash?".into(),
                lines:vec!["Permanently empty the Trash? This cannot be undone.".into()],
                pending:Pending::EmptyTrash(self.trash_base.clone()),
            }),
            "placerename"=>if let Some(place)=self.context_place.and_then(|i|self.places.get(i)).cloned() {
                let len=place.name.chars().count();
                self.rename_target=Some(place.path.clone());
                self.begin_edit(EditMode::PlaceName,place.name.clone(),Some((0,len)));
            },
            "placeremove"=>if let Some(path)=self.context_place_path() {self.mutate_bookmark(&path,|items,index|bookmarks::remove(items,index).is_some());},
            "connect"=>{self.begin_edit(EditMode::Path,"sftp://".into(),None);self.model.status="Type sftp://user@host/path, fish://user@host/path or ssh://user@host and press Enter".into();},
            "hidden"=>self.toggle_hidden(),
            "showtype"=>self.toggle_show_type(),
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
        self.context=None;
        if paths.iter().any(|p|remote::is_remote(p)) {self.confirm_remote_delete(paths);return;}
        match ops::validate(ops::Kind::Delete,&source_facts(&paths),None,true) {
            Verdict::Run(paths)=>{let location=parent_dir(&paths);self.confirm_items("Delete permanently?",&paths,&location,Pending::Delete(paths.clone()));},
            Verdict::NoOp=>{},
            Verdict::Refuse(message)=>self.model.status=message.into(),
        }
    }
    /// Same Confirm dialog as local deletes: item name and the remote folder, then a recursive remote remove.
    fn confirm_remote_delete(&mut self,paths:Vec<PathBuf>) {
        if paths.is_empty() {return;}
        let name=match paths.as_slice() {
            [only]=>only.file_name().unwrap_or(only.as_os_str()).to_string_lossy().into_owned(),
            many=>format!("{} items",many.len()),
        };
        let location=paths.first().and_then(|p|remote::url_of(p)).and_then(|url|url.parent()).map(|parent|remote::display_text(&parent)).unwrap_or_default();
        self.dialog=Some(Dialog::Confirm{title:"Delete permanently?".into(),lines:vec![name,location],pending:Pending::RemoteDelete(paths)});
    }
    fn request_trash(&mut self,paths:Vec<PathBuf>) {
        self.context=None;
        match ops::validate(ops::Kind::Trash,&source_facts(&paths),None,true) {
            Verdict::Run(paths)=>{let location=parent_dir(&paths);self.confirm_items("Move to trash?",&paths,&location,Pending::Trash(paths.clone()));},
            Verdict::NoOp=>{},
            Verdict::Refuse(message)=>self.model.status=message.into(),
        }
    }
    /// The one reusable confirmation: title, item name (or "N items"), and the location shortened to `~`.
    fn confirm_items(&mut self,title:&str,paths:&[PathBuf],location:&Path,pending:Pending) {
        let name=match paths {
            [only]=>only.file_name().unwrap_or(only.as_os_str()).to_string_lossy().into_owned(),
            many=>format!("{} items",many.len()),
        };
        let lines=vec![name,ops::display_location(location,&home_dir())];
        self.dialog=Some(Dialog::Confirm{title:title.into(),lines,pending});
    }
    /// Button slots: primary, secondary, tertiary, cancel. Each dialog kind decides what they mean.
    fn dialog_action(&mut self,slot:&str) {
        // Permissions are edited in the address field; Enter applies, Escape cancels that edit only.
        if let (Some(Dialog::Properties{props,..}),"primary")=(self.dialog.as_ref(),slot) {
            self.field.set(format!("{:o}",props.mode&0o7777));
            self.edit=Some(EditMode::Mode);
            return;
        }
        let Some(dialog)=self.dialog.take() else {return};
        match (dialog,slot) {
            (Dialog::Properties{cancel,..},_)=>cancel.store(true,Ordering::SeqCst),
            (Dialog::Confirm{pending,..},"primary")=>self.run_pending(pending),
            (Dialog::Confirm{..},_)=>{},
            (Dialog::HostKey{url,prompt:Some(prompt)},"primary")=>self.save_host_key(url,prompt),
            (Dialog::HostKey{..},_)=>{},
            (Dialog::AuthFailed{url,..},"primary")=>if self.tab_shows(&url) {self.scan();},
            (Dialog::AuthFailed{..},_)=>{},
            (Dialog::Conflicts{sources,directory,move_it,..},"primary")=>self.send_transfer(sources,directory,move_it,Conflict::Skip),
            (Dialog::Conflicts{sources,directory,move_it,..},"secondary")=>self.send_transfer(sources,directory,move_it,Conflict::KeepBoth),
            (Dialog::Conflicts{sources,directory,move_it,count},"tertiary")=>{
                self.dialog=Some(Dialog::Confirm{
                    title:"Replace existing items?".into(),
                    lines:vec![format!("Replace {count} existing item(s)? Replaced items are not recoverable.")],
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
            Pending::Trash(paths)=>{
                let len=paths.len();
                if self.tx.send(WorkerJob::Trash{paths}).is_ok() {self.busy=true;self.model.status=format!("Moving {len} item(s) to Trash…");}
                else {self.model.status="Trash worker unavailable".into();}
            },
            Pending::EmptyTrash(base)=>{
                if self.tx.send(WorkerJob::EmptyTrash{base}).is_ok() {self.busy=true;self.model.status="Emptying Trash…".into();}
            },
            Pending::Transfer{sources,directory,move_it,conflict}=>self.send_transfer(sources,directory,move_it,conflict),
            Pending::Drop{sources,directory,force_copy}=>{
                if self.busy {self.model.status="File operation already in progress".into();return;}
                let text=if force_copy {"Copying dropped items…"} else {"Transferring dropped items…"};
                if self.tx.send(WorkerJob::Drop{sources,directory,force_copy,cancel:self.cancel.clone()}).is_ok() {
                    self.busy=true;self.progress=Some(0);self.activity=None;self.model.status=text.into();
                } else {self.model.status="Transfer worker unavailable".into();}
            },
            Pending::RemoteCopy{sources,directory}=>self.start_remote_copy(sources,directory),
            Pending::RemoteDelete(paths)=>{
                let urls:Vec<RemoteUrl>=paths.iter().filter_map(|p|remote::url_of(p)).collect();
                if !urls.is_empty() {self.send_remote(RemoteOp::Delete{urls},"Deleting…");}
            },
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
    /// Top and height of the scrollbar track: the list area between the header and the status strip.
    fn scrollbar_track(&self)->(f32,f32) {
        let top=list_top(self.model.tabs.len()>1);
        (top,(self.window.1-STATUS_STRIP_HEIGHT-top).max(0.0))
    }
    /// Entries in the list, counting search results while a search is active.
    fn scroll_total(&self)->usize {
        match self.search_rows.as_ref() {
            Some(results)=>results.len(),
            None=>self.model.tab().filtered_count(),
        }
    }
    /// Column dividers move with the pointer in window coordinates. Each motion applies only its own delta.
    fn column_grip(&mut self,divider:Divider,e:&UiActionEvent) {
        match e.phase {
            UiActionPhase::Press if e.button==1 && e.inside => self.col_drag=Some((divider,e.x)),
            UiActionPhase::Motion => if let Some((active,last))=self.col_drag {
                let total=self.window.0-SIDEBAR_WIDTH;
                self.widths.drag_divider(active,e.x-last,total,self.show_type);
                self.col_drag=Some((active,e.x));
            },
            UiActionPhase::Release if self.col_drag.is_some() => {
                self.col_drag=None;
                self.save_prefs();
            },
            _=>{},
        }
    }
    /// Thumb drag moves the list by pixel ratio, snapping without glide so the rows track the pointer.
    /// Track clicks page one window towards the click.
    fn scrollbar_event(&mut self,action:&str,e:&UiActionEvent) {
        let (track_top,track_h)=self.scrollbar_track();
        let rows=self.visible_rows();
        let total=self.scroll_total();
        match e.phase {
            UiActionPhase::Press if e.button==1 && e.inside => {
                let origin=self.origin(rows);
                if action=="explorer.scroll.thumb" {
                    self.scroll_drag=Some((e.y,origin));
                } else if let Some((thumb_top,_))=scroll::thumb_geometry(track_h,total,rows,origin) {
                    let step=if e.y<track_top+thumb_top {-(rows as isize)} else {rows as isize};
                    self.set_origin((origin as isize+step).clamp(0,total.saturating_sub(rows) as isize) as usize,rows);
                }
            },
            UiActionPhase::Motion => if let Some((start_y,start_origin))=self.scroll_drag {
                let target=scroll::drag_origin(track_h,total,rows,start_origin,e.y-start_y);
                self.set_origin(target,rows);
            },
            UiActionPhase::Release => self.scroll_drag=None,
            _=>{},
        }
    }
    /// Sets the window origin without a glide; the drag and track paths follow the pointer directly.
    fn set_origin(&mut self,origin:usize,rows:usize) {
        self.model.tab_mut().scroll=origin;
        self.glide=RowGlide::default();
        self.scroll_origin=self.origin(rows);
    }
    fn event(&mut self,e:&UiActionEvent) {
        let action=e.action.as_str();
        if let Some(divider)=action.strip_prefix("explorer.colgrip.") {
            let divider=match divider {"mod"=>Some(Divider::Modified),"size"=>Some(Divider::Size),"kind"=>Some(Divider::Kind),_=>None};
            if let Some(divider)=divider {self.column_grip(divider,e);}
            return;
        }
        if action=="explorer.scroll.track"||action=="explorer.scroll.thumb" {
            self.scrollbar_event(action,e);
            return;
        }
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
        // Inline renames commit when the pointer goes down elsewhere; the overflow button handles its own toggle.
        if e.phase==UiActionPhase::Press && !action.starts_with("explorer.menu") {
            if matches!(self.edit,Some(EditMode::Rename|EditMode::PlaceName)) {
                let content=self.field.text.clone();self.field.set(String::new());
                if let Some(edit)=self.edit.take() {self.commit_edit(edit,&content);}
            }
            // Pressing anywhere outside an open menu closes it.
            if self.context.is_some() {self.context=None;self.menu_overflow=false;self.context_place=None;}
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
                    let outside=!(0.0..self.window.0).contains(&e.x)||!(0.0..self.window.1).contains(&e.y);                    if active && outside {
                        // Leaving the window turns the row drag into an XDND drag of the same files.
                        if let Some(drag)=self.row_drag.take() {
                            self.drop_folder=None;
                            self.file_drag_out=Some(self.dragged_sources(&drag.source));
                        }
                    } else if active {
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
                    UiActionPhase::Press if e.button==1 && e.inside && (SIDEBAR_WIDTH..).contains(&e.x) => {
                        self.context=None;
                        let p=self.clamp_to_list(e.x,e.y);
                        self.rubber=Some((p,p));
                    },
                    UiActionPhase::Motion => if let Some((start,_))=self.rubber {
                        let p=self.clamp_to_list(e.x,e.y);
                        self.rubber=Some((start,p));
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
            "explorer.search"=>{let text=self.model.tab().search.clone();self.begin_edit(EditMode::Search,text,None);},
            "explorer.search.clear"=>{self.edit=None;self.clear_search();self.search_text.clear();self.model.status="Search cleared".into();},
            "explorer.address"=>{let text=self.model.tab().path().to_string_lossy().into_owned();self.begin_edit(EditMode::Path,text,None);},
            "explorer.tab.new"=>{if self.model.tabs.len()<5 {self.model.new_tab();self.scan();}},
            "explorer.tab.close"=>{self.model.close_tab();self.scan();},
            "explorer.sort.name"=>self.sort_by(Column::Name),
            "explorer.sort.size"=>self.sort_by(Column::Size),
            "explorer.sort.modified"=>self.sort_by(Column::Modified),
            "explorer.sort.kind"=>self.sort_by(Column::Type),
            "explorer.menu"=>{
                // The overflow button toggles: a second click on it closes the menu.
                if self.context.is_some() && self.menu_overflow {self.context=None;self.menu_overflow=false;}
                else {self.context_on_entry=false;self.menu_overflow=true;self.context=Some((self.window.0-8.0,53.0));}
            },
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
            if name=="escape" {self.edit=None;self.rename_target=None;self.context_place=None;return;}
            if name=="return"||name=="enter" {
                let content=std::mem::take(&mut self.field.text);self.field.set(String::new());self.edit=None;
                self.commit_edit(edit,&content);
                return;
            }
            if ctrl && name=="a" {self.field.select_all();return;}
            match name.as_str() {
                "backspace"=>self.field.backspace(),
                "delete"=>self.field.delete(),
                "left"=>{let to=self.field.caret.saturating_sub(1);self.field.move_caret(to);},
                "right"=>{let to=self.field.caret+1;self.field.move_caret(to);},
                "keysym(0xff50)"=>self.field.move_caret(0),
                "keysym(0xff57)"=>self.field.move_caret(usize::MAX),
                _=>if let Some(text)=&key.text {if !ctrl && !text.chars().any(char::is_control) {self.field.insert(text);}},
            }
            return;
        }
        if let Some(text)=key.text.as_deref().filter(|t|!ctrl && t.chars().count()==1 && !t.chars().any(char::is_control)) {
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
            "delete" if shift || remote::is_remote(self.model.tab().path())=>self.confirm_delete(),
            "delete"=>self.action_menu("trash"),
            "return"|"enter" if alt=>if let Some(path)=self.selected(){self.open_properties(path)},
            "backspace"=>{if self.model.tab_mut().up(){self.scan();}},
            "escape"=>{if self.busy {self.cancel.cancel();self.model.status="Cancelling…".into();return;}if self.search_rows.is_some() {self.clear_search();self.model.status="Search cleared".into();return;}self.context=None;self.rubber=None;self.row_drag=None;self.drop_folder=None;self.last_click=None;self.model.tab_mut().clear_selection();},
            "return"|"enter"=>if let Some(path)=self.selected(){self.open(&path)},
            "left" if alt=>{if self.model.tab_mut().back(){self.scan();}},
            "right" if alt=>{if self.model.tab_mut().forward(){self.scan();}},
            "l" if ctrl=>{let text=self.model.tab().path().to_string_lossy().into_owned();self.begin_edit(EditMode::Path,text,None);},
            "f" if ctrl=>{let text=self.model.tab().search.clone();self.begin_edit(EditMode::Search,text,None);},
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
            WorkerResult::RemoteRows{generation,path,mut rows}=>{
                // Local listings filter hidden names in `list_directory`; remote chunks are filtered here.
                if !self.model.hidden {rows.retain(|entry|!entry.name.starts_with('.'));}
                if self.model.append_listing(generation,&path,rows) {
                    view::sort_entries(&mut self.model.tab_mut().entries,self.column_sort);
                }
            },
            WorkerResult::RemoteDone{generation,path,failure}=>{
                if self.model.finish_listing(generation,&path) {
                    match failure {
                        Some(message)=>self.model.status=message,
                        None=>if let Some(url)=remote::url_of(&path) {
                            remote::remember(&mut self.remote_recent,&url);
                            self.save_remote_recent();
                        },
                    }
                }
            },
            WorkerResult::RemoteBlocked{generation,path,error}=>{
                if self.model.finish_listing(generation,&path) {self.remote_blocked(path,error);}
            },
            WorkerResult::HostKeyProbed{url,result}=>match result {
                Ok(prompt)=>if let Some(Dialog::HostKey{url:open,prompt:slot})=self.dialog.as_mut() {
                    if open.to_string()==url.to_string() {*slot=Some(prompt);self.model.status="Host key received: compare the fingerprint before trusting".into();}
                },
                Err(message)=>{
                    if matches!(self.dialog,Some(Dialog::HostKey{..})) {self.dialog=None;}
                    self.model.status=message;
                },
            },
            WorkerResult::HostKeyTrusted{url,result}=>match result {
                Ok(())=>{
                    self.model.status="Host key saved; connecting…".into();
                    if self.tab_shows(&url) {self.scan();}
                },
                Err(message)=>self.model.status=message,
            },
            WorkerResult::RemoteDownloaded(result)=>match result {
                Ok(local)=>self.open(&local),
                Err(message)=>self.model.status=message,
            },
            WorkerResult::DirChanged=>{
                if !self.busy && self.dialog.is_none() {self.request_listing();}
            },
            WorkerResult::ScrollTick=>self.step_scroll(),
            WorkerResult::ResizeFlush=>self.flush_resize(),
            WorkerResult::Thumbnail{key,pixels}=>self.thumbs.borrow_mut().complete(key,pixels),
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
            WorkerResult::OpenResolved{path,result}=>match result {
                Ok(true)=>{},
                Ok(false)=>self.request_open_with(path),
                Err(e)=>self.model.status=format!("Open failed: {e}"),
            },
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
        let address=match self.edit {Some(EditMode::Path)=>self.field.display(),Some(EditMode::Mode)=>format!("Permissions: {}|",self.field.text),_=>display_location(tab.path(),&home_dir())};
        doc.text("address-text",address)?;
        doc.background("address-text",if matches!(self.edit,Some(EditMode::Path)) && self.field.has_selection() {self.theme.selection_fill()} else {self.theme.clear()})?;
        // Toolbar fields have fixed widths (the layout engine cannot shrink them): drop the path bar first, then search, so they never overlap.
        doc.visible("address",self.window.0>=480.0)?;
        doc.visible("search-toggle",self.window.0>=400.0)?;
        // The field shows only the query: a plain placeholder when empty, and the clear button while a search is active.
        let search=match self.edit {
            Some(EditMode::Search)=>self.field.display(),
            _ if self.search_rows.is_some()=>self.search_text.clone(),
            _=>"Search".to_owned(),
        };
        doc.text("search-text",search)?;
        doc.background("search-text",if matches!(self.edit,Some(EditMode::Search)) && self.field.has_selection() {self.theme.selection_fill()} else {self.theme.clear()})?;
        doc.visible("search-clear",self.search_rows.is_some())?;
        doc.text("menu-connect-label","Connect to Server…".to_string())?;
        doc.text("menu-hidden-label",format!("Show Hidden Files: {}",if self.model.hidden {"On"} else {"Off"}))?;
        doc.text("menu-showtype-label",format!("Show Type Column: {}",if self.show_type {"On"} else {"Off"}))?;
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
                    // Inline place rename: the label becomes the field.
                    let label=if matches!(self.edit,Some(EditMode::PlaceName)) && self.rename_target.as_ref()==Some(&place.path) {self.field.display()} else {place.name.clone()};
                    doc.text(&format!("place-label-{slot}"),label)?;
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
        for (key,x,shown) in [("mod",mod_x,true),("size",size_x,true),("kind",kind_x,kind_shown)] {
            doc.position(&format!("col-sep-{key}"),x,0.0)?;
            doc.visible(&format!("col-sep-{key}"),shown)?;
            doc.position(&format!("col-grip-{key}"),x-COLUMN_GRIP_HALF,0.0)?;
            doc.visible(&format!("col-grip-{key}"),shown)?;
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
        for (column,key) in [(Column::Name,"name"),(Column::Modified,"mod"),(Column::Size,"size"),(Column::Type,"kind")] {
            let active=self.column_sort.column==column;
            doc.visible(&format!("sort-{key}-asc"),active&&!self.column_sort.descending)?;
            doc.visible(&format!("sort-{key}-desc"),active&&self.column_sort.descending)?;
        }
        self.project_rows(doc,true)?;
        // Scrollbar: drawn only while some entries are outside the window; thumb size and position follow the window.
        let rows=self.visible_rows();
        let origin=self.origin(rows);
        let (track_top,track_h)=self.scrollbar_track();
        let track_x=self.window.0-SCROLLBAR_TRACK_WIDTH;
        match scroll::thumb_geometry(track_h,self.scroll_total(),rows,origin) {
            Some((thumb_top,thumb_len))=>{
                doc.position("scroll-track",track_x,track_top)?;
                doc.size("scroll-track",SCROLLBAR_TRACK_WIDTH,track_h)?;
                doc.visible("scroll-track",true)?;
                doc.position("scroll-thumb",track_x+SCROLLBAR_THUMB_INSET,track_top+thumb_top)?;
                doc.size("scroll-thumb",SCROLLBAR_THUMB_WIDTH,thumb_len)?;
                doc.visible("scroll-thumb",true)?;
            },
            None=>{
                doc.visible("scroll-track",false)?;
                doc.visible("scroll-thumb",false)?;
            },
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
        // Empty list space under the rows is the background context surface (rubber band and background menu).
        doc.visible("list-surface",true)?;
        let surface_top=list_top(self.model.tabs.len()>1);
        let surface_bottom=self.window.1-STATUS_STRIP_HEIGHT;
        doc.position("list-surface",SIDEBAR_WIDTH,surface_top)?;
        doc.size("list-surface",(self.window.0-SIDEBAR_WIDTH).max(1.0),(surface_bottom-surface_top).max(1.0))?;
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
                    "open"|"newtab"|"properties"=>self.context_place.is_some(),
                    "emptytrash"=>self.context_place_path().is_some_and(|path|path==self.trash_base.join("files")),
                    _=>false,
                }
            } else {match item {
                // Folder-only actions: a file entry offers none; a folder entry offers its own; the background offers the current folder.
                "newfolder"=>!self.context_on_entry,
                "terminal"|"bookmark"|"newtab"=>!self.context_on_entry || self.selected_folder().is_some(),
                "paste"=>true,
                "emptytrash"=>in_trash,
                "extract"=>self.ark && self.context_on_entry && !many && selected_path.as_deref().is_some_and(is_archive),
                "compress"=>self.ark && self.context_on_entry && selected_path.is_some(),
                "copy"|"cut"|"trash"|"duplicate"=>self.context_on_entry && selected_path.is_some(),
                "open"|"openwith"|"rename"|"properties"=>self.context_on_entry && !many && selected_path.is_some(),
                "opencontaining"=>searching && self.context_on_entry && !many && selected_path.is_some(),
                "restore"=>in_trash && self.context_on_entry && selected_path.is_some(),
                "hidden"|"showtype"|"connect"=>self.menu_overflow,
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
    /// Shows the row's decoded thumbnail, or the type icon until it is ready. A slot is rebound only when its image id changes.
    /// List rows glide: slot k draws window row k-1 at its eased offset; the outer slots are off-window until they settle.
    /// A glide frame (`full == false`) only moves rows and toggles their visibility: the origin is unchanged, so each
    /// slot keeps the text, icon and background of the last full projection.
    fn project_rows(&self, doc:&mut impl UiDocumentAccess, full:bool)->Result<(),String> {
        let tab=self.model.tab();
        let kind_shown=self.show_type;
        let rows=self.visible_rows();
        let origin=self.origin(rows);
        let top=list_top(self.model.tabs.len()>1);
        let bottom=self.window.1-STATUS_STRIP_HEIGHT;
        let glide=self.glide.offset(Instant::now());
        for slot in 0..ROW_SLOTS {
            let row=format!("row-{slot}");
            let index=flamewm_explorer::row_slot_index(origin,slot);
            let y=top+(slot as f32-1.0+glide)*ROW_HEIGHT;
            let entry=if slot<=rows+1 {self.entry_at(index)} else {None};
            let in_view=y>=top-0.5&&y+ROW_HEIGHT<=bottom+0.5;
            if let Some(entry)=entry {
                // Contents are set even off-view, so a row that glides into view later already shows its entry.
                doc.visible(&row,in_view)?;
                doc.position(&row,SIDEBAR_WIDTH,y)?;
                if !full {continue;}
                doc.size(&row,self.window.0-SIDEBAR_WIDTH,ROW_HEIGHT)?;
                doc.text(&format!("name-{slot}"),entry.name.clone())?;
                doc.text(&format!("size-{slot}"),entry.size_label())?;
                doc.text(&format!("modified-{slot}"),entry.modified_label())?;
                doc.text(&format!("kind-{slot}"),view::kind_name(entry))?;
                doc.size(&format!("modified-{slot}"),self.widths.modified,ROW_HEIGHT)?;
                doc.size(&format!("size-{slot}"),self.widths.size,ROW_HEIGHT)?;
                doc.size(&format!("kind-{slot}"),self.widths.kind,ROW_HEIGHT)?;
                doc.visible(&format!("kind-{slot}"),kind_shown)?;
                // Dot files show the dimmed name twin (`name-dim-*`, CSS colour). Foreground overrides cannot reach a
                // text child, and the icon raster is shared, so the icon stays full strength.
                let dim=entry.name.starts_with('.');
                doc.visible(&format!("name-{slot}"),!dim)?;
                doc.visible(&format!("name-dim-{slot}"),dim)?;
                doc.text(&format!("name-dim-{slot}"),entry.name.clone())?;
                // Inline rename: the name cell shows the field with its caret; a selected stem is highlighted.
                if matches!(self.edit,Some(EditMode::Rename)) && self.rename_target.as_ref()==Some(&entry.path) {
                    doc.visible(&format!("name-{slot}"),true)?;
                    doc.visible(&format!("name-dim-{slot}"),false)?;
                    doc.text(&format!("name-{slot}"),self.field.display())?;
                    doc.background(&format!("name-{slot}"),if self.field.has_selection() {self.theme.selection_fill()} else {self.theme.clear()})?;
                } else {doc.background(&format!("name-{slot}"),self.theme.clear())?;}
                self.project_row_icon(doc,slot,entry)?;
                doc.background(&row,if self.drop_folder.as_deref()==Some(entry.path.as_path()) {
                    self.theme.selected()
                } else if tab.selected_paths.contains(&entry.path){self.theme.selected()}
                    else if index%2==0{self.theme.clear()}else{self.theme.row_alternate()})?;
            } else {doc.visible(&row,false)?;}
        }
        Ok(())
    }
    fn project_row_icon(&self, doc:&mut impl UiDocumentAccess, slot:usize, entry:&Entry)->Result<(),String> {
        let mut bound=self.thumb_bound.borrow_mut();
        match self.row_thumbnail(entry) {
            Some((id,pixels))=>{
                if bound[slot]!=Some(id) {
                    doc.image_rgba8_shared(&format!("ico-{slot}-thumb"),format!("explorer-thumb-{id}"),THUMB_SIZE,THUMB_SIZE,pixels)?;
                    bound[slot]=Some(id);
                }
                for kind in ROW_ICONS {doc.visible(&format!("ico-{slot}-{kind}"),false)?;}
                doc.visible(&format!("ico-{slot}-thumb"),true)?;
            }
            None=>{
                bound[slot]=None;
                doc.visible(&format!("ico-{slot}-thumb"),false)?;
                let icon=view::file_icon(entry);
                for kind in ROW_ICONS {doc.visible(&format!("ico-{slot}-{kind}"),kind==icon)?;}
            }
        }
        Ok(())
    }
    /// Cached thumbnail for an image row. A cache miss queues one decode and shows the type icon meanwhile.
    fn row_thumbnail(&self, entry:&Entry)->Option<(u64,Arc<Vec<u8>>)> {
        if remote::is_remote(&entry.path) {return None;}
        let key=ThumbKey::of(entry)?;
        let mut cache=self.thumbs.borrow_mut();
        match cache.lookup(&key) {
            Lookup::Ready(id,pixels)=>Some((id,pixels)),
            Lookup::Failed=>None,
            Lookup::Missing=>{
                if cache.begin(&key) {
                    if let Some(tx)=&self.thumb_tx {let _=tx.send(key);}
                }
                None
            }
        }
    }
    fn project_dialog(&self, doc:&mut impl UiDocumentAccess)->Result<(),String> {
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
            for slot in 0..CHOICE_ROWS {
                let id=format!("openwith-choice-{slot}");
                if let Some((index,choice))=choices.get(slot) {
                    doc.visible(&id,true)?;
                    let suffix=if choice.is_default {" · Default"} else if !choice.associated {" · Other app"} else {""};
                    doc.text(&format!("openwith-label-{slot}"),format!("{}{suffix}",choice.entry.name()))?;
                    let icon_id=format!("openwith-icon-{slot}");
                    match choice.icon.as_ref() {
                        Some(icon)=>{
                            // Rows bind their icon by application id, so a slot reused for another app rebinds on change.
                            let source=format!("explorer-app-icon-{}",choice.entry.id().as_str());
                            doc.image_rgba8_shared(&icon_id,source,icon.width,icon.height,Arc::clone(&icon.pixels))?;
                            doc.visible(&icon_id,true)?;
                        }
                        None=>doc.visible(&icon_id,false)?,
                    }
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
    flamewm_profiler::init_process("flamewm-explorer");
    if flamewm_profiler::enabled() {
        // No reactor timer in this process: a plain thread drains the span window every interval.
        let interval=std::time::Duration::from_secs(flamewm_profiler::profile_interval_secs());
        std::thread::spawn(move||loop{std::thread::sleep(interval);let _=flamewm_profiler::report_window();});
    }
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
    let (thumb_tx,thumb_jobs)=mpsc::channel::<ThumbKey>();
    let thumb_results=result_tx.clone();
    let thumb_wake=writer.try_clone().map_err(|e|e.to_string())?;
    std::thread::Builder::new().name("flamewm-explorer-thumbs".into()).spawn(move||thumb_worker(thumb_jobs,thumb_results,thumb_wake)).map_err(|e|e.to_string())?;
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
        app.thumb_tx=Some(thumb_tx);
        app.search=Some(search);
        let settings=initial_settings();
        app.theme=settings.as_ref().map_or_else(ExplorerTheme::default,ExplorerTheme::from_snapshot);
        backdrop=settings.as_ref().map(|snapshot|theme::window_backdrop(snapshot,&packaged_root()));
        let saved=prefs::load(&prefs::config_dir());
        app.model.hidden=saved.show_hidden;
        app.show_type=saved.show_type;
        app.widths=Widths{modified:saved.col_modified as f32,size:saved.col_size as f32,kind:saved.col_kind as f32}.clamped();
    }
    app.borrow_mut().scan();
    // Present-but-unmounted devices arrive from lsblk on a worker; without this they only show after a reload.
    app.borrow().refresh_devices();
    let mut doc=decode_document(UI)?.with_file_drop_target();
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
            let _event_span=flamewm_profiler::start(match event {
                UiControllerEvent::Resized{..}=>"explorer.event.resized",
                UiControllerEvent::Action(_)=>"explorer.event.action",
                UiControllerEvent::Key(_)=>"explorer.event.key",
                _=>"explorer.event.other",
            });
            match event {
                UiControllerEvent::Action(action)=>{
                    app.event(action);
                    app.sync_scroll();
                    // A wheel notch during a glide only moves the target: the next frame tick projects the burst, so
                    // N notches cost one relayout per frame instead of N.
                    if action.action=="scroll" && app.glide.is_animating(Instant::now()) {return Ok(());}
                },
                UiControllerEvent::Key(key)=>{app.key(key);app.sync_scroll();},
                // Settled client size: sizes, scroll window and rows all follow from `window` in project.
                UiControllerEvent::Resized{width,height}=>{
                    // Held sizes are projected by the trailing ResizeFlush wake, not on every event.
                    if !app.resize(*width,*height) {return Ok(());}
                },
                UiControllerEvent::Drag(UiDragEvent::Drop{payload:flamewm_ui_x11::DropPayload::ArkExtract{service,path},..})=>app.ark_drop(service.clone(),path.clone()),
                UiControllerEvent::Drag(UiDragEvent::Drop{paths,x,y,..})=>app.external_drop(paths.clone(),*x,*y),
                // Enter/over/leave do not change the list layout.
                _=>{},
            }
            // A row dragged out of the window starts the XDND drag here, once the event is handled.
            if let Some(paths)=app.file_drag_out.take() {
                if let Err(error)=document.begin_file_drag(paths) {app.model.status=error;}
            }
            app.list_dirty=false;
            app.project(document)
        },
        move |document|{
            let mut app=wake_app.borrow_mut();
            let snapshot_changed=match wake_snapshots.borrow_mut().take(){Some(snapshot)=>{app.theme=ExplorerTheme::from_snapshot(&snapshot);true},None=>false};
            // Every X wake (a pure window move included) lands here. Project only when something this
            // closure owns changed: a worker message, a settings snapshot, the list origin, or a running glide.
            let mut changed=snapshot_changed;
            while let Ok(message)=results.try_recv(){app.wake(message);changed=true;}
            for rx in std::mem::take(&mut app.ark_extracts) {
                match rx.try_recv() {
                    Ok(Ok(report)) => { app.model.status=format!("Extracted {} item(s) from Ark",report.created.len()); app.scan(); }
                    Ok(Err(error)) => { app.model.status=format!("Ark extraction failed: {error:?}"); }
                    Err(std::sync::mpsc::TryRecvError::Empty) => app.ark_extracts.push(rx),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => { app.model.status="Ark extraction ended unexpectedly".into(); }
                }
            }
            app.sync_scroll();
            changed|=std::mem::take(&mut app.list_dirty);
            if changed {return app.project(document);}
            // A glide frame moves the rows only: no text, icon or chrome relayout.
            if app.glide.is_animating(Instant::now()) {return app.project_rows(document,false);}
            Ok(())
        },
    )
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

/// Filesystem facts for the guard; a source that cannot be stat'ed is reported missing.
fn source_facts(paths:&[PathBuf])->Vec<ops::Source> {
    paths.iter().map(|path|{
        let meta=fs::symlink_metadata(path).ok();
        ops::Source{path:path.clone(),exists:meta.is_some(),is_dir:meta.as_ref().is_some_and(|m|m.is_dir())}
    }).collect()
}
/// Parent directory of the first path, used as the location line of a confirmation.
fn parent_dir(paths:&[PathBuf])->PathBuf {
    paths.first().and_then(|p|p.parent()).unwrap_or(Path::new("/")).to_path_buf()
}

/// Converts `flamewm-remote-fs` listing chunks on this thread and forwards them to the UI loop. A panic
/// becomes a visible failure instead of a silent stop.
fn forward_listing(generation:u64,path:PathBuf,url:RemoteUrl,events:Receiver<ListEvent>,tx:Sender<WorkerResult>,mut wake:UnixStream) {
    let outcome=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
        loop {
            match events.recv() {
                Ok(ListEvent::Chunk(items))=>{
                    let rows=items.iter().map(|item|remote::entry_of(&url,item)).collect();
                    send_wake(&mut wake,&tx,WorkerResult::RemoteRows{generation,path:path.clone(),rows});
                },
                Ok(ListEvent::Done{..})=>{
                    send_wake(&mut wake,&tx,WorkerResult::RemoteDone{generation,path:path.clone(),failure:None});
                    return;
                },
                Ok(ListEvent::Failed(flamewm_remote_fs::Error::Cancelled))=>return,
                Ok(ListEvent::Failed(error))=>{
                    if matches!(error,flamewm_remote_fs::Error::HostKeyUnknown{..}|flamewm_remote_fs::Error::AuthFailed{..}) {
                        send_wake(&mut wake,&tx,WorkerResult::RemoteBlocked{generation,path:path.clone(),error});
                    } else {
                        send_wake(&mut wake,&tx,WorkerResult::RemoteDone{generation,path:path.clone(),failure:Some(remote::describe(&error))});
                    }
                    return;
                },
                Err(_)=>{
                    send_wake(&mut wake,&tx,WorkerResult::RemoteDone{generation,path:path.clone(),failure:Some("Remote listing ended unexpectedly".into())});
                    return;
                },
            }
        }
    }));
    if outcome.is_err() {
        send_wake(&mut wake,&tx,WorkerResult::RemoteDone{generation,path,failure:Some("Remote listing failed internally".into())});
    }
}

/// Waits for a remote download and reports the local cache path, or the error, to the UI loop.
fn forward_download(local:PathBuf,transfers:Receiver<TransferEvent>,tx:Sender<WorkerResult>,mut wake:UnixStream) {
    let outcome=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||->Result<PathBuf,String>{
        loop {
            match transfers.recv() {
                Ok(TransferEvent::Done{..})=>return Ok(local.clone()),
                Ok(TransferEvent::Progress{..})=>{},
                Ok(TransferEvent::Failed(error))=>return Err(remote::describe(&error)),
                Err(_)=>return Err("Download ended unexpectedly".into()),
            }
        }
    }));
    let result=outcome.unwrap_or_else(|_|Err("Download failed internally".into()));
    send_wake(&mut wake,&tx,WorkerResult::RemoteDownloaded(result));
}

