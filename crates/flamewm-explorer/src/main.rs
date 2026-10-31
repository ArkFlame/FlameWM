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
use flamewm_integrations_linux::device_watch::DeviceWatch;
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
const MENU_ITEMS: [&str;26]=["connect","showtype","open","openwith","setwallpaper","cut","copy","paste","rename","duplicate","newfolder","bookmark","trash","properties","terminal","compress","extract","newtab","opencontaining","restore","emptytrash","placerename","placeremove","unmount","eject","hidden"];
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
/// Side of the square that holds a row's icon (`.row-ico` in explorer.css, and `.row-thumb`, which
/// inherits the same box). The type glyph and the thumbnail are laid out in this exact square, so a
/// row never reflows when a thumbnail arrives and the glyph is never squeezed out of the cell.
const ROW_ICON_BOX:f32=18.0;
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
    /// Unmounted filesystems from lsblk.
    Devices(Vec<bookmarks::BlockDevice>),
    /// A block device or mount-table change arrived from the kernel; the sidebar reloads.
    DevicesChanged,
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
    /// Outcome of appending the confirmed host key to `known_hosts`.
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
mod workers;
use workers::*;
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
    menu_overflow:bool, context_scroll:usize, glide:RowGlide, scroll_origin:usize, scroll_ticking:bool,
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
    search_rows:Option<Vec<Entry>>, watch:Option<DirWatch>, watched:Option<PathBuf>, device_watch:Option<DeviceWatch>,
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
mod app {
    mod lifecycle;
    mod editing;
    mod navigation;
    mod transfers;
    mod places;
    mod menus;
    mod scroll;
    mod input;
    mod projection;
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
pub fn main(){if let Err(error)=run(){eprintln!("flamewm-explorer: {error}");std::process::exit(1);}}
fn run()->Result<(),String>{
    flamewm_profiler::init_process("flamewm-explorer");
    if flamewm_profiler::enabled() {
        // No reactor timer in this process: a plain thread drains the span window every interval.
        let interval=std::time::Duration::from_secs(flamewm_profiler::profile_interval_secs());
        std::thread::spawn(move||loop{std::thread::sleep(interval);let _=flamewm_profiler::report_window();});
    }
    let args=std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.iter().any(|a|matches!(a.to_string_lossy().as_ref(),"--help"|"-h")) {println!("FlameWM Explorer — usage: flamewm-explorer [DIRECTORY]");return Ok(());}
    let path=args.first().map_or_else(home_dir, PathBuf::from);
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
    app.borrow_mut().watch_devices();
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
                UiControllerEvent::Drag(_)=>"explorer.event.other",
                UiControllerEvent::Visibility{..}=>"explorer.event.other",
            });
            match event {
                UiControllerEvent::Action(action)=>{
                    let rubber_before=app.rubber;
                    app.event(action);
                    app.sync_scroll();
                    // A wheel notch during a glide only moves the target: the next frame tick projects the burst, so
                    // N notches cost one relayout per frame instead of N.
                    if action.action=="scroll" && app.glide.is_animating(Instant::now()) {return Ok(());}
                    // A hover is a pointer position, not a state change: the Explorer paints no hover
                    // state, so every pointer move over a row would otherwise re-project the whole window.
                    let pointer_drag=app.row_drag.is_some()||app.rubber.is_some()||app.col_drag.is_some()||app.scroll_drag.is_some();
                    if hover_skips_projection(action.phase,app.list_dirty,pointer_drag) {return Ok(());}
                    // A rubber drag changes only the overlay until release (rows commit on Release).
                    if rubber_motion_only(action.phase,app.list_dirty,app.rubber.is_some(),app.row_drag.is_some()||app.col_drag.is_some()||app.scroll_drag.is_some()) {
                        // Sub-pixel pointer moves clamp to the same rubber corners: the overlay is already right.
                        if app.rubber==rubber_before {return Ok(());}
                        return app.project_rubber(document);
                    }
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
                UiControllerEvent::Drag(_)=>{},
                UiControllerEvent::Visibility{..}=>{},
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


/// A hover moves no Explorer state, so it needs no projection unless a list change is already pending.
/// A hover, or a motion with no drag in progress, changes no state, so it does not re-project the window.
fn hover_skips_projection(phase:UiActionPhase,list_dirty:bool,pointer_drag:bool)->bool {
    !list_dirty && (phase==UiActionPhase::Hover || (phase==UiActionPhase::Motion && !pointer_drag))
}

/// Motion during a pure rubber-band drag (no row, column or scroll drag) with a clean list.
fn rubber_motion_only(phase:UiActionPhase,list_dirty:bool,rubber:bool,other_drag:bool)->bool {
    !list_dirty && phase==UiActionPhase::Motion && rubber && !other_drag
}

#[cfg(test)]
#[path = "tests/internal_drag_tests.rs"]
mod internal_drag_tests;

#[cfg(test)]
#[path = "tests/window_frame_tests.rs"]
mod window_frame_tests;

/// Which image fills a row's icon slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowIcon {
    /// A decoded thumbnail, by cache id.
    Thumbnail(u64),
    /// The embedded type glyph for `view::file_icon`.
    Type(&'static str),
}

/// A row shows the thumbnail only when one is ready in the cache; every other state (queued, failed,
/// empty canvas, non-image row) resolves to a compiled family glyph. Keeping this pure makes the
/// "never blank" contract testable without a live document.
fn row_icon_choice(entry:&Entry, thumbnail:Option<&(u64,Arc<Vec<u8>>)>)->RowIcon {
    match thumbnail {
        Some((id,_))=>RowIcon::Thumbnail(*id),
        None=>RowIcon::Type(view::file_icon(entry)),
    }
}

#[cfg(test)]
#[path = "tests/icon_slot_tests.rs"]
mod icon_slot_tests;

/// Filesystem facts for the guard; a source that cannot be stat'ed is reported missing.
fn source_facts(paths:&[PathBuf])->Vec<ops::Source> {
    paths.iter().map(|path|{
        let meta=fs::symlink_metadata(path).ok();
        ops::Source{path:path.clone(),exists:meta.is_some(),is_dir:meta.as_ref().is_some_and(std::fs::Metadata::is_dir)}
    }).collect()
}
/// Parent directory of the first path, used as the location line of a confirmation.
fn parent_dir(paths:&[PathBuf])->PathBuf {
    paths.first().and_then(|p|p.parent()).unwrap_or(Path::new("/")).to_path_buf()
}

/// Converts `flamewm-remote-fs` listing chunks on this thread and forwards them to the UI loop.
fn forward_listing(generation:u64,path:PathBuf,url:RemoteUrl,events:Receiver<ListEvent>,tx:Sender<WorkerResult>,mut wake:UnixStream) {
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
}

/// Waits for a remote download and reports the local cache path, or the error, to the UI loop.
fn forward_download(local:PathBuf,transfers:Receiver<TransferEvent>,tx:Sender<WorkerResult>,mut wake:UnixStream) {
    let result:Result<PathBuf,String>=loop {
        match transfers.recv() {
            Ok(TransferEvent::Done{..})=>break Ok(local.clone()),
            Ok(TransferEvent::Progress{..})=>{},
            Ok(TransferEvent::Failed(error))=>break Err(remote::describe(&error)),
            Err(_)=>break Err("Download ended unexpectedly".into()),
        }
    };
    send_wake(&mut wake,&tx,WorkerResult::RemoteDownloaded(result));
}

