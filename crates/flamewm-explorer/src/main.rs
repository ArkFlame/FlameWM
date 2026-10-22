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

use flamewm_explorer::{AddressTarget,BatchReport,Entry,Explorer,Place,SortBy,PAGE_ROWS,add_bookmark,display_location,home_dir,is_archive,list_directory,parse_address,places};
use flamewm_explorer::view::{self,Column,ColumnSort,ViewMode};
use flamewm_explorer::{archive,properties,refresh::DirWatch,search::SearchWorker,trash,places as bookmarks};
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
const MENU_ITEMS: [&str;22]=["open","openwith","cut","copy","paste","rename","duplicate","newfolder","bookmark","trash","properties","terminal","compress","extract","newtab","opencontaining","restore","emptytrash","placerename","placeremove","unmount","eject"];
/// Home, six user folders and System come before user bookmarks in `places()`.
const BUILTIN_PLACES:usize=8;
// Canonical geometry: paint (project) and hit-testing (cell_rect, folder_at, select bands) all derive from these.
const TOOLBAR_HEIGHT:f32=45.0;
const TABBAR_HEIGHT:f32=36.0;
const HEADER_HEIGHT:f32=29.0;
const ROW_HEIGHT:f32=35.0;
const SIDEBAR_WIDTH:f32=201.0;
const DRAG_THRESHOLD_PX:f32=7.0;
// Maximum window size; the actual size is clamped to 90% of the root (see `window_size`).
const WINDOW_WIDTH:u32=1060;
const WINDOW_HEIGHT:u32=720;

/// Window size: the maximum size, clamped to 90% of the root so small screens still fit.
fn window_size(root:(u32,u32))->(u32,u32) {
    (WINDOW_WIDTH.min((u64::from(root.0)*9/10) as u32).max(1),WINDOW_HEIGHT.min((u64::from(root.1)*9/10) as u32).max(1))
}
/// Top of the column header: below the toolbar and, with more than one tab, the tab strip.
fn header_top(tab_strip:bool)->f32 {TOOLBAR_HEIGHT+if tab_strip {TABBAR_HEIGHT} else {0.0}}
/// Top of the first row, directly below the column header.
fn list_top(tab_strip:bool)->f32 {header_top(tab_strip)+HEADER_HEIGHT}
fn kind_label(entry:&Entry)->String {
    if entry.directory {return "Folder".into();}
    if entry.symlink {return "Link".into();}
    match Path::new(&entry.name).extension().and_then(|e|e.to_str()) {
        Some(ext) if !ext.is_empty()=>format!("{} file",ext.to_uppercase()),
        _=>"File".into(),
    }
}
const ICON_COLUMNS:usize=4;
const ICON_WIDTH:f32=200.0;
const ICON_HEIGHT:f32=28.0;
const ICON_GAP:f32=8.0;
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
    /// Search worker has a new poll result; the app drains it on the UI thread.
    SearchTick,
    /// Directory watcher saw a change in the listed folder.
    DirChanged,
    ChooserReady { request: u64, chooser: Result<OpenWithChooser, String> },
    DefaultSaved(Result<(), String>),
    BatchCompleted {report:BatchReport,action:&'static str,cut_sources:Option<Vec<PathBuf>>},
    Completed(Result<String,String>),
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
                let home=home_dir();
                let data=std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".local/share"));
                let uid=fs::read_to_string("/proc/self/status").ok()
                    .and_then(|s|flamewm_desktop_core::trash::current_uid_from_proc(&s));
                let mut report=BatchReport::default();
                for path in &paths {
                    let outcome=if let Some(uid)=uid {
                        Trash::move_to_trash(path,&home,&data,uid)
                            .map(|item|Undo::Trashed{original:item.original,stored:item.stored,info:item.info})
                            .map_err(|e|std::io::Error::other(e.to_string()))
                    } else {
                        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"cannot determine current user"))
                    };
                    match outcome {
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
                let mut journal=jobs::journal().lock().unwrap_or_else(|poison|poison.into_inner());
                let done=match journal.take_last() {
                    None=>Err("Nothing to undo".to_owned()),
                    Some(entry)=>match jobs::undo(&entry) {
                        Ok(())=>Ok("Undone".to_owned()),
                        Err(error)=>{journal.restore(entry);Err(format!("Undo failed: {error}"))},
                    },
                };
                drop(journal);
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
    view:ViewMode, column_sort:ColumnSort, typed:String, typed_at:Option<Instant>,
    dialog:Option<Dialog>, ark:bool, search:Option<SearchWorker>, search_id:u64,
    search_rows:Option<Vec<Entry>>, watch:Option<DirWatch>, watched:Option<PathBuf>,
    context_place:Option<usize>, progress_total:u64, trash_base:PathBuf,
    /// Wake channel for background threads (watcher, search, size). Set once in `run`.
    events:Option<(Sender<WorkerResult>,UnixStream)>,
}
impl ExplorerApp {
    fn new(initial:PathBuf,tx:Sender<WorkerJob>,scan_tx:Sender<WorkerJob>,chooser_tx:Sender<ChooserJob>) -> Self {
        let home=home_dir();
        let trash_base=std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".local/share")).join("Trash");
        let mut app=Self {model:Explorer::new(initial),tx,scan_tx,chooser_tx,chooser:None,chooser_loading:false,chooser_request:0,places:Vec::new(),context:None,edit:None,buffer:String::new(),theme:ExplorerTheme::default(),window:(WINDOW_WIDTH as f32,WINDOW_HEIGHT as f32),last_click:None,progress:None,context_on_entry:false,rubber:None,activity:None,busy:false,row_drag:None,drop_folder:None,cancel:CancellationToken::new(),view:ViewMode::Details,column_sort:ColumnSort::default(),typed:String::new(),typed_at:None,
            dialog:None,ark:archive::find_ark().is_some(),search:None,search_id:0,search_rows:None,watch:None,watched:None,context_place:None,progress_total:0,trash_base,events:None};
        app.reload_places();
        app
    }
    /// Built-in and bookmark places, plus the Trash folder just before mounted devices.
    fn reload_places(&mut self) {
        let home=home_dir();
        let mut list=places(&home);
        let trash=Place{name:"Trash".into(),path:self.trash_base.join("files"),removable:false};
        let at=list.iter().position(|p|p.removable).unwrap_or(list.len());
        list.insert(at,trash);
        self.places=list;
    }
    fn clear_search(&mut self) {
        self.search_rows=None;
        if let Some(search)=self.search.as_mut() {search.cancel();}
    }
    fn start_search(&mut self,query:String) {
        if query.trim().is_empty() {self.clear_search();self.model.status="Search cleared".into();return;}
        let root=self.model.tab().path().to_path_buf();
        let hidden=self.model.hidden;
        let started=self.search.as_mut().map(|search|search.start(root,query,hidden));
        match started {
            Some(Ok(id))=>{self.search_id=id;self.search_rows=None;self.model.status="Searching…".into();},
            Some(Err(error))=>self.model.status=format!("Search unavailable: {error}"),
            None=>self.model.status="Search worker unavailable".into(),
        }
    }
    fn take_search_results(&mut self) {
        let Some(result)=self.search.as_mut().and_then(|search|search.poll()) else {return};
        if result.id!=self.search_id {return;}
        let rows=result.outcome.hits.into_iter().map(|hit|Entry{path:hit.path,name:hit.name,directory:hit.directory,symlink:false,size:0,modified:None}).collect::<Vec<_>>();
        self.model.status=format!("{} result(s){}",rows.len(),if result.outcome.truncated {" (truncated)"} else {""});
        self.search_rows=Some(rows);
    }
    /// Entries shown in the file pane: search results replace the folder listing while active.
    fn visible_entries(&self)->Vec<&Entry> {
        if let Some(rows)=self.search_rows.as_ref() {return rows.iter().take(PAGE_ROWS).collect();}
        let tab=self.model.tab();
        let page=tab.page.min(tab.filtered_count().saturating_sub(1)/PAGE_ROWS);
        tab.visible_page_at(page)
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
            let tab=self.model.tab_mut();
            tab.select_one(path);
            tab.page=index/PAGE_ROWS;
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
    fn chosen(&self,slot:usize) -> Option<&Entry> {
        self.visible_entries().get(slot).copied()
    }
    /// Window-space rectangle of one file slot. Details and Compact are list rows; Icons is a grid.
    fn cell_rect(&self,slot:usize)->(f32,f32,f32,f32) {
        let list_top=list_top(self.model.tabs.len()>1);
        if self.view==ViewMode::Icons {
            let column=(slot%ICON_COLUMNS) as f32;
            let row=(slot/ICON_COLUMNS) as f32;
            return (SIDEBAR_WIDTH+ICON_GAP+column*(ICON_WIDTH+ICON_GAP),list_top+ICON_GAP+row*(ICON_HEIGHT+ICON_GAP),ICON_WIDTH,ICON_HEIGHT);
        }
        (SIDEBAR_WIDTH,list_top+ROW_HEIGHT*slot as f32,self.window.0-SIDEBAR_WIDTH,ROW_HEIGHT)
    }
    fn selected(&self) -> Option<PathBuf> {self.model.tab().selected.clone()}
    /// Drop coordinates belong to the Explorer surface. Only visible folder rows
    /// inside the file pane are eligible; no guessed external-window targets.
    fn folder_at(&self,x:f32,y:f32)->Option<PathBuf> {
        // Icon-grid cells are not rows, so only the list layouts map pointer y to a slot.
        if self.view==ViewMode::Icons || !x.is_finite() || !y.is_finite() || !(SIDEBAR_WIDTH..self.window.0).contains(&x) {return None;}
        let top=list_top(self.model.tabs.len()>1);
        if y<top || y>=top+ROW_HEIGHT*PAGE_ROWS as f32 {return None;}
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
    fn undo_last(&mut self) {
        if self.busy {self.model.status="File operation already in progress".into();return;}
        if self.tx.send(WorkerJob::Undo).is_ok() {self.busy=true;self.model.status="Undoing…".into();}
        else {self.model.status="File worker unavailable".into();}
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
                    self.model.clipboard=Some((paths,cmd=="cut"));
                }
            },
            "paste"=>if let Some((sources,cut))=self.model.clipboard.clone() {self.start_transfer(sources,cut);},
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
            "bookmark"=>{let path=self.model.tab().path().to_path_buf();match add_bookmark(&path,&home_dir()) {Ok(())=>{self.places=places(&home_dir());self.model.status="Added to Places".into();},Err(e)=>self.model.status=format!("Bookmark failed: {e}")}},
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
            "unmount"|"eject"=>if let Some(path)=self.context_place_path() {self.device_action(cmd,&path);},
            _=>{}
        }
        self.context=None;
        self.context_place=None;
    }
    fn context_place_path(&self)->Option<PathBuf> {
        self.context_place.and_then(|i|self.places.get(i)).map(|p|p.path.clone())
    }
    fn is_bookmark(&self,index:usize)->bool {
        self.places.get(index).is_some_and(|p|!p.removable && index>=BUILTIN_PLACES && p.path!=self.trash_base.join("files"))
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
    /// Unmount or eject a mounted removable device found through mountinfo.
    fn device_action(&mut self,verb:&str,mount_point:&Path) {
        let source=fs::read_to_string("/proc/self/mountinfo").ok()
            .and_then(|text|bookmarks::parse_mountinfo(&text).into_iter().find(|d|d.mount_point==mount_point).map(|d|d.source));
        let Some(source)=source else {self.model.status="Device is no longer mounted".into();self.reload_places();return;};
        match bookmarks::udisks_action(verb,&source) {
            Ok(_)=>{self.model.status=format!("{} {}",if verb=="eject"{"Ejected"}else{"Unmounted"},mount_point.display());self.reload_places();},
            Err(error)=>self.model.status=format!("{verb} failed: {error}"),
        }
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
                    if let Some(entry)=self.chosen(slot) {
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
        if action=="explorer.surface" {
            if e.button==3 && e.phase==UiActionPhase::Release && e.inside {
                self.context_on_entry=false;
                self.context=Some((e.x,e.y));
                self.rubber=None;
            } else {
                match e.phase {
                    UiActionPhase::Press if e.button==1 && e.inside && self.view!=ViewMode::Icons => {
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
                                let tab=self.model.tab_mut();
                                tab.select_vertical_band(start.min(PAGE_ROWS),end.min(PAGE_ROWS));
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
            if let Some(entry)=self.chosen(slot).cloned() {
                let path=entry.path;
                if e.button == 3 {
                    // Context-click on a selected file preserves the full batch selection.
                    if !self.model.tab().selected_paths.contains(&path) {
                        self.model.tab_mut().select_one(path.clone());
                    }
                    self.last_click=None;
                    self.context_on_entry=true;
                    self.context_place=None;
                    self.context=Some((e.x,e.y));
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
        if let Some(slot)=action.strip_prefix("explorer.place.").and_then(|v|v.parse::<usize>().ok()) {
            if e.button==3 {
                // Right-click on a sidebar item opens the place menu instead of navigating.
                self.context_on_entry=false;
                self.context_place=Some(slot);
                self.context=Some((e.x,e.y));
            } else if let Some(place)=self.places.get(slot) {self.navigate(place.path.clone());}
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
            "explorer.reload"=>{self.places=places(&home_dir());self.scan();},
            "explorer.search"=>{self.edit=Some(EditMode::Search);self.buffer=self.model.tab().search.clone();},
            "explorer.address"=>{self.edit=Some(EditMode::Path);self.buffer=self.model.tab().path().to_string_lossy().into_owned();},
            "explorer.tab.new"=>{if self.model.tabs.len()<5 {self.model.new_tab();self.scan();}},
            "explorer.tab.close"=>{self.model.close_tab();self.scan();},
            "explorer.page.prev"=>{let tab=self.model.tab_mut();tab.page=tab.page.saturating_sub(1);},
            "explorer.page.next"=>{let tab=self.model.tab_mut();tab.page=(tab.page+1).min(tab.page_count()-1);},
            "explorer.view.icons"=>self.view=ViewMode::Icons,
            "explorer.view.details"=>self.view=ViewMode::Details,
            "explorer.sort.name"=>self.sort_by(Column::Name),
            "explorer.sort.size"=>self.sort_by(Column::Size),
            "explorer.sort.modified"=>self.sort_by(Column::Modified),
            "explorer.menu"=>{self.context_on_entry=false;self.context=Some((725.0,53.0));},
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
            "h" if ctrl=>{self.model.hidden=!self.model.hidden;self.scan();},
            "1"|"2"|"3" if ctrl=>self.view=ViewMode::from_shortcut(name.as_bytes()[0]-b'0').unwrap_or(self.view),
            "a" if ctrl=>self.model.tab_mut().select_all_filtered(),
            "space"|" " if ctrl=>if let Some(path)=self.selected() {self.model.tab_mut().toggle_selection(path)},
            "c" if ctrl=>self.action_menu("copy"),
            "x" if ctrl=>self.action_menu("cut"),
            "v" if ctrl=>self.action_menu("paste"),
            "z" if ctrl=>self.undo_last(),
            "up"=>self.step_selection(-1,shift),
            "down"=>self.step_selection(1,shift),
            _=>{}
        }
    }
    fn step_selection(&mut self,delta:isize,extend:bool){
        let tab=self.model.tab();
        let count=tab.filtered_count();
        if count==0{return;}
        let current=self.selected().as_ref().and_then(|p|tab.visible_index_of(p)).unwrap_or(0);
        let next=(current as isize+delta).clamp(0,count as isize-1) as usize;
        let path=tab.visible_nth(next).expect("index within filtered count").path.clone();
        let page=next/PAGE_ROWS;
        let tab=self.model.tab_mut();
        if extend {tab.select_range_to(path)}else{tab.select_one(path)}
        tab.page=page;
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
            WorkerResult::SearchTick=>self.take_search_results(),
            WorkerResult::SizeReady{path,bytes}=>{
                if let Some(Dialog::Properties{props,size,..})=self.dialog.as_mut() {
                    if props.path==path {*size=Some(bytes.unwrap_or(0));}
                }
            },
            WorkerResult::Progress(bytes)=>{self.progress=Some(bytes);},
            WorkerResult::Total(bytes)=>{self.progress_total=bytes;},
            WorkerResult::BatchCompleted{report,action,cut_sources}=>{
                self.busy=false;
                self.cancel=CancellationToken::new();
                if let Some(sources)=cut_sources.as_ref() {
                    if self.model.clipboard.as_ref().is_some_and(|(paths,cut)| *cut && paths==sources) {
                        // For a partial move, keep only the failed sources for a safe retry.
                        self.model.clipboard=(!report.failed_sources.is_empty())
                            .then(||(report.failed_sources.clone(),true));
                    }
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
        let address=match self.edit {Some(EditMode::Path)=>format!("{}|",self.buffer),Some(EditMode::Search)=>format!("Search: {}|",self.buffer),Some(EditMode::Rename)=>format!("Rename: {}|",self.buffer),Some(EditMode::Mode)=>format!("Permissions: {}|",self.buffer),Some(EditMode::PlaceName)=>format!("Place name: {}|",self.buffer),None=>display_location(tab.path(),&home_dir())};
        doc.text("address",address)?;
        doc.foreground("nav-back",if tab.can_back(){WHITE}else{MUTED})?;
        doc.foreground("nav-forward",if tab.can_forward(){WHITE}else{MUTED})?;
        // Native Finder-style chrome: a single tab has no visible tab strip.
        let tab_strip=self.model.tabs.len()>1;
        doc.visible("tabs",tab_strip)?;
        doc.visible("file-pager",tab.page_count()>1)?;
        for i in 0..5 {
            let exists=tab_strip && i<self.model.tabs.len();
            doc.visible(&format!("tab-{i}"),exists)?;
            if exists {
                let title=self.model.tabs[i].path().file_name().unwrap_or_default().to_string_lossy().into_owned();
                doc.text(&format!("tab-{i}"),if title.is_empty(){"System".into()}else{title})?;
                doc.background(&format!("tab-{i}"),if i==self.model.active{self.theme.selected()}else{self.theme.card()})?;
            }
        }
        for i in 0..15 {
            if let Some(place)=self.places.get(i) {
                doc.visible(&format!("place-{i}"),true)?;
                doc.text(&format!("place-label-{i}"),place.name.clone())?;
                doc.background(&format!("place-{i}"),if place.path==tab.path(){self.theme.selected()}else{self.theme.sidebar()})?;
            } else {doc.visible(&format!("place-{i}"),false)?;}
        }
        let count=tab.filtered_count();
        let page=tab.page.min(count.saturating_sub(1)/PAGE_ROWS);
        let entries=self.visible_entries();
        // Only Details shows the metadata columns; Icons shows names alone.
        let details=self.view==ViewMode::Details;
        let arrow=|column:Column,label:&str|if self.column_sort.column==column {
            format!("{label} {}",if self.column_sort.descending{"v"}else{"^"})
        } else {label.to_owned()};
        // The header and every row share one geometry: flow cells with identical fixed widths.
        doc.position("columns",SIDEBAR_WIDTH,header_top(tab_strip))?;
        doc.size("columns",self.window.0-SIDEBAR_WIDTH,HEADER_HEIGHT)?;
        doc.visible("columns",details)?;
        doc.text("col-name",arrow(Column::Name,"Name"))?;
        doc.text("col-mod",arrow(Column::Modified,"Date modified"))?;
        doc.text("col-size",arrow(Column::Size,"Size"))?;
        doc.text("col-kind","Kind".to_owned())?;
        for slot in 0..PAGE_ROWS {
            let row=format!("row-{slot}");
            if let Some(entry)=entries.get(slot) {
                doc.visible(&row,true)?;
                let (left,top,width,height)=self.cell_rect(slot);
                doc.position(&row,left,top)?;
                doc.size(&row,width,height)?;
                doc.text(&format!("name-{slot}"),entry.name.clone())?;
                doc.text(&format!("size-{slot}"),entry.size_label())?;
                doc.text(&format!("modified-{slot}"),entry.modified_label())?;
                doc.text(&format!("kind-{slot}"),kind_label(entry))?;
                doc.visible(&format!("size-{slot}"),details)?;
                doc.visible(&format!("modified-{slot}"),details)?;
                doc.visible(&format!("kind-{slot}"),details)?;
                doc.background(&row,if self.drop_folder.as_deref()==Some(entry.path.as_path()) {
                    self.theme.selected()
                } else if tab.selected_paths.contains(&entry.path){self.theme.selected()}
                    else if slot%2==0{self.theme.surface()}else{self.theme.row_alternate()})?;
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
        doc.text("page-label",format!("{} / {}",page+1,tab.page_count()))?;
        let status=if self.row_drag.as_ref().is_some_and(|drag|drag.dragging) {
            if let Some(folder)=self.drop_folder.as_deref() {
                format!("Drop into {} · Move (Ctrl to copy)",folder.file_name().unwrap_or_default().to_string_lossy())
            } else {"Drag onto a folder to move · Hold Ctrl to copy".into()}
        } else {self.activity.as_ref().unwrap_or(&self.model.status).clone()};
        doc.text("status-message",status)?;
        let selected_bytes=tab.entries.iter().filter(|e|tab.selected_paths.contains(&e.path)).map(|e|e.size).sum::<u64>();
        doc.text("status-count",view::status_text(tab.selected_paths.len(),selected_bytes,tab.entries.len(),view::free_space(tab.path())))?;
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
                    "unmount"|"eject"=>self.context_place.is_some_and(|i|self.places.get(i).is_some_and(|p|p.removable)),
                    _=>false,
                }
            } else {match item {
                "terminal"|"newtab"|"newfolder"|"bookmark"=>true,
                "paste"=>self.model.clipboard.is_some(),
                "emptytrash"=>in_trash,
                "extract"=>self.ark && self.context_on_entry && !many && selected_path.as_deref().is_some_and(is_archive),
                "compress"=>self.ark && self.context_on_entry && selected_path.is_some(),
                "copy"|"cut"|"trash"|"duplicate"=>self.context_on_entry && selected_path.is_some(),
                "open"|"openwith"|"rename"|"properties"=>self.context_on_entry && !many && selected_path.is_some(),
                "opencontaining"=>searching && self.context_on_entry && !many && selected_path.is_some(),
                "restore"=>in_trash && self.context_on_entry && selected_path.is_some(),
                _=>false,
            }};
            doc.visible(&format!("menu-{item}"),enabled)?;
            if enabled {visible_rows+=1;}
        }
        if let Some((x,y))=self.context {
            // Shared desktop context-menu metrics; hidden actions consume no space.
            let metrics=flamewm_ui_core::context_menu::MenuMetrics::canonical();
            let height=visible_rows as f32*metrics.row_height as f32+2.0*metrics.padding as f32;
            doc.position("context-popup",x.min(self.window.0-210.0).max(0.0),y.min(self.window.1-height).max(0.0))?;
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
        doc.background("dialog",self.theme.card())?;
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
        doc.background("openwith-dialog",self.theme.card())?;
        doc.background("openwith-toolbar",self.theme.surface())?;
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
const MUTED:UiColor=UiColor{r:120,g:125,b:133,a:255};

fn initial_theme()->ExplorerTheme {
    let Ok(client)=ControlClient::connect(BusKind::Session) else {return ExplorerTheme::default()};
    match client.call_with_timeout(&ControlRequest::GetSettings,Duration::from_millis(300)) {
        Ok(ControlResponse::Settings(snapshot))=>ExplorerTheme::from_snapshot(&snapshot),
        _=>ExplorerTheme::default(),
    }
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
    let (window_w,window_h)=window_size(flamewm_ui_x11::root_geometry().unwrap_or((u32::MAX,u32::MAX)));
    let app=Rc::new(RefCell::new(ExplorerApp::new(directory,tx,scan_tx,chooser_tx)));
    {
        let mut app=app.borrow_mut();
        app.window=(window_w as f32,window_h as f32);
        app.events=Some(app_events);
        app.search=Some(search);
        app.theme=initial_theme();
    }
    app.borrow_mut().scan();
    let mut doc=decode_document(UI)?;
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
        UiWindowConfig{width:window_w,height:window_h,x:80,y:65,title:"FlameWM Explorer".into(),identity:None},
        UiWindowRole::Normal,fd,
        move||{
            let mut bytes=[0_u8;1024];
            loop {match reader.read(&mut bytes){Ok(0)=>return UiWakeDrain::Retire,Ok(_)=>{},Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>return UiWakeDrain::Keep,Err(_)=>return UiWakeDrain::Retire}}
        },notifications,
        move |event,document|{
            let mut app=event_app.borrow_mut();
            match event {UiControllerEvent::Action(action)=>app.event(action),UiControllerEvent::Key(key)=>app.key(key)}
            app.project(document)
        },
        move |document|{
            let mut app=wake_app.borrow_mut();
            if let Some(snapshot)=wake_snapshots.borrow_mut().take(){app.theme=ExplorerTheme::from_snapshot(&snapshot);}
            while let Ok(message)=results.try_recv(){app.wake(message);}
            app.project(document)
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
mod layout_tests {
    use super::*;
    fn app(view:ViewMode) -> ExplorerApp {
        let (tx,_rx)=mpsc::channel();
        let (scan_tx,_scan_rx)=mpsc::channel();
        let (chooser_tx,_chooser_rx)=mpsc::channel();
        let mut app=ExplorerApp::new(PathBuf::from("/"),tx,scan_tx,chooser_tx);
        app.view=view;
        app
    }
    #[test]
    fn icon_grid_wraps_after_four_cells_and_list_rows_stack() {
        let icons=app(ViewMode::Icons);
        let (x0,y0,_,_)=icons.cell_rect(0);
        let (x4,y4,_,_)=icons.cell_rect(4);
        assert_eq!(x4,x0);
        assert!(y4>y0);
        assert!(icons.cell_rect(3).0>x0);
        let list=app(ViewMode::Compact);
        let (_,a,_,h)=list.cell_rect(0);
        let (_,b,_,_)=list.cell_rect(1);
        assert_eq!(b-a,ROW_HEIGHT);
        assert_eq!(h,ROW_HEIGHT);
    }
}
