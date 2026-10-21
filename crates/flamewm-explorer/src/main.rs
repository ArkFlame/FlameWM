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
use flamewm_explorer::jobs::{self,CancellationToken,Conflict,Undo};
use theme::ExplorerTheme;
use flamewm_control_core::{ControlRequest,ControlResponse};
use flamewm_control_dbus::{ControlClient,ControlSignal,ControlSignalClient};
use flamewm_dbus_reactor::BusKind;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_desktop_core::{file_actions::create_new_folder,trash::Trash};
use flamewm_ui_x11::{UiActionEvent,UiActionPhase,UiColor,UiControllerEvent,UiDocumentAccess,UiKeyEvent,UiKeyPhase,UiWakeDrain,UiWindowConfig,UiWindowRole,run_with_controller_events_role_and_two_readable_fds,decode_document};

const UI: &[u8]=include_bytes!(concat!(env!("OUT_DIR"),"/flamewm-explorer.rwr"));
const MENU_ITEMS: [&str;15]=["open","openwith","cut","copy","paste","rename","duplicate","newfolder","bookmark","trash","properties","terminal","compress","extract","newtab"];
const TOOLBAR_HEIGHT:f32=48.0;
const TABBAR_HEIGHT:f32=36.0;
const COLUMNS_HEIGHT:f32=30.0;
const FILE_ROW_HEIGHT:f32=26.0;
const FILE_LIST_LEFT:f32=204.0;
const DRAG_THRESHOLD_PX:f32=7.0;

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
    Transfer {sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,cancel:CancellationToken},
    Drop {sources:Vec<PathBuf>,directory:PathBuf,force_copy:bool,cancel:CancellationToken},
    Trash {paths:Vec<PathBuf>},
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
            WorkerJob::Transfer{sources,directory,move_it,cancel}=>{
                let mut accumulated=0_u64;
                let mut last_progress=Instant::now();
                let mut progress=|size:u64|{
                    accumulated=accumulated.saturating_add(size);
                    if last_progress.elapsed()>=Duration::from_millis(100) {
                        send_wake(&mut wake,&result,WorkerResult::Progress(accumulated));
                        last_progress=Instant::now();
                    }
                };
                let mut report=jobs::transfer_batch_with(&sources,&directory,move_it,Conflict::KeepBoth,&cancel,&mut progress);
                jobs::record(report.undo.drain(..));
                send_wake(&mut wake,&result,WorkerResult::BatchCompleted {
                    report,action:if move_it {"Moved"}else{"Copied"},
                    cut_sources:move_it.then_some(sources),
                });
            }
            WorkerJob::Drop{sources,directory,force_copy,cancel}=>{
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
#[derive(Clone,Copy)]
enum EditMode { Path, Search, Rename }
struct ExplorerApp {
    model:Explorer, tx:Sender<WorkerJob>, scan_tx:Sender<WorkerJob>,
    chooser_tx:Sender<ChooserJob>, chooser:Option<OpenWithChooser>,
    chooser_loading:bool, chooser_request:u64, places:Vec<Place>,
    context:Option<(f32,f32)>, edit:Option<EditMode>, buffer:String, theme:ExplorerTheme,
    last_click:Option<(PathBuf,Instant)>, progress:Option<u64>,
    context_on_entry:bool, rubber:Option<((f32,f32),(f32,f32))>, activity:Option<String>, busy:bool,
    row_drag:Option<RowDrag>, drop_folder:Option<PathBuf>, cancel:CancellationToken,
}
impl ExplorerApp {
    fn new(initial:PathBuf,tx:Sender<WorkerJob>,scan_tx:Sender<WorkerJob>,chooser_tx:Sender<ChooserJob>) -> Self {
        Self {model:Explorer::new(initial),tx,scan_tx,chooser_tx,chooser:None,chooser_loading:false,chooser_request:0,places:places(&home_dir()),context:None,edit:None,buffer:String::new(),theme:ExplorerTheme::default(),last_click:None,progress:None,context_on_entry:false,rubber:None,activity:None,busy:false,row_drag:None,drop_folder:None,cancel:CancellationToken::new()}
    }
    fn scan(&mut self) {
        self.row_drag=None;
        self.drop_folder=None;
        self.activity=None;
        let generation=self.model.next_generation();
        let path=self.model.tab().path().to_path_buf();
        self.model.status="Loading directory…".into();
        if self.scan_tx.send(WorkerJob::Scan{generation,path,hidden:self.model.hidden,sort:self.model.sort}).is_err() {
            self.model.status="File worker unavailable".into();
        }
    }
    fn navigate(&mut self,path:PathBuf) {
        if path.is_dir() {self.model.tab_mut().visit(path);self.scan();} else {self.model.status="Directory does not exist".into();}
        self.context=None;
    }
    fn chosen(&self,slot:usize) -> Option<&Entry> {
        let tab=self.model.tab();
        tab.visible_nth(tab.page.saturating_mul(PAGE_ROWS)+slot)
    }
    fn selected(&self) -> Option<PathBuf> {self.model.tab().selected.clone()}
    /// Drop coordinates belong to the Explorer surface. Only visible folder rows
    /// inside the file pane are eligible; no guessed external-window targets.
    fn folder_at(&self,x:f32,y:f32)->Option<PathBuf> {
        if !x.is_finite() || !y.is_finite() || !(FILE_LIST_LEFT..1060.0).contains(&x) {return None;}
        let top=TOOLBAR_HEIGHT+if self.model.tabs.len()>1 {TABBAR_HEIGHT}else{0.0}+COLUMNS_HEIGHT;
        if y<top || y>=top+FILE_ROW_HEIGHT*PAGE_ROWS as f32 {return None;}
        let slot=((y-top)/FILE_ROW_HEIGHT) as usize;
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
        self.progress=Some(0);
        self.activity=None;
        self.model.status=format!("{} {} item(s)…",if move_it{"Moving"}else{"Copying"},sources.len());
        if self.tx.send(WorkerJob::Transfer{sources,directory,move_it,cancel:self.cancel.clone()}).is_err() {
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
            "properties"=>if let Some(path)=selected {match fs::symlink_metadata(&path) {
                Ok(m)=>self.model.status=format!("{} · {} bytes · permissions {:o}",path.display(),m.len(),std::os::unix::fs::PermissionsExt::mode(&m.permissions())&0o777),
                Err(e)=>self.model.status=format!("Properties: {e}")
            }},
            _=>{}
        }
        self.context=None;
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
                    UiActionPhase::Press if e.button==1 && e.inside => {
                        self.context=None;
                        self.rubber=Some(((e.x,e.y),(e.x,e.y)));
                    },
                    UiActionPhase::Motion => if let Some((start,_))=self.rubber {
                        self.rubber=Some((start,(e.x,e.y)));
                    },
                    UiActionPhase::Release => {
                        if let Some(((x0,y0),(x1,y1)))=self.rubber.take() {
                            let top=TOOLBAR_HEIGHT+(if self.model.tabs.len()>1 {TABBAR_HEIGHT}else{0.0})+COLUMNS_HEIGHT;
                            if (x1-x0).abs()>5.0 || (y1-y0).abs()>5.0 {
                                let start=((y0.min(y1)-top)/FILE_ROW_HEIGHT).floor().max(0.0) as usize;
                                let end=((y0.max(y1)-top)/FILE_ROW_HEIGHT).floor().max(0.0) as usize;
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
            if let Some(place)=self.places.get(slot) {self.navigate(place.path.clone());} return;
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
            "explorer.sort.name"=>{self.model.sort=SortBy::Name;self.scan();},
            "explorer.sort.size"=>{self.model.sort=SortBy::Size;self.scan();},
            "explorer.sort.modified"=>{self.model.sort=SortBy::Modified;self.scan();},
            "explorer.menu"=>{self.context_on_entry=false;self.context=Some((725.0,53.0));},
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
                    EditMode::Search=>{let tab=self.model.tab_mut();tab.search=content;tab.page=0;},
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
        match name.as_str() {
            "f5"=>self.scan(),
            "f2"=>self.action_menu("rename"),
            "delete"=>self.action_menu("trash"),
            "backspace"=>{if self.model.tab_mut().up(){self.scan();}},
            "escape"=>{if self.busy {self.cancel.cancel();self.model.status="Cancelling…".into();return;}self.context=None;self.rubber=None;self.row_drag=None;self.drop_folder=None;self.last_click=None;self.model.tab_mut().clear_selection();},
            "return"|"enter"=>if let Some(path)=self.selected(){self.open(&path)},
            "left" if alt=>{if self.model.tab_mut().back(){self.scan();}},
            "right" if alt=>{if self.model.tab_mut().forward(){self.scan();}},
            "l" if ctrl=>{self.edit=Some(EditMode::Path);self.buffer=self.model.tab().path().to_string_lossy().into_owned();},
            "f" if ctrl=>{self.edit=Some(EditMode::Search);self.buffer=self.model.tab().search.clone();},
            "t" if ctrl=>{if self.model.tabs.len()<5{self.model.new_tab();self.scan();}},
            "w" if ctrl=>{self.model.close_tab();self.scan();},
            "h" if ctrl=>{self.model.hidden=!self.model.hidden;self.scan();},
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
            WorkerResult::Scanned{generation,path,entries}=>{self.model.apply_listing(generation,&path,entries);},
            WorkerResult::Progress(bytes)=>{self.progress=Some(bytes);},
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
        let address=match self.edit {Some(EditMode::Path)=>format!("{}|",self.buffer),Some(EditMode::Search)=>format!("Search: {}|",self.buffer),Some(EditMode::Rename)=>format!("Rename: {}|",self.buffer),None=>display_location(tab.path(),&home_dir())};
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
        let entries=tab.visible_page_at(page);
        for slot in 0..PAGE_ROWS {
            let row=format!("row-{slot}");
            if let Some(entry)=entries.get(slot) {
                doc.visible(&row,true)?;
                let name=if entry.directory{format!("▸  {}",entry.name)}else{format!("    {}",entry.name)};
                doc.text(&format!("name-{slot}"),name)?;
                doc.text(&format!("size-{slot}"),entry.size_label())?;
                doc.text(&format!("modified-{slot}"),entry.modified_label())?;
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
        let selected_count=tab.selected_paths.len();
        doc.text("status-count",if selected_count>0 {format!("{selected_count} selected · {} items",tab.entries.len())}
            else {format!("{} folders, {} files",tab.entries.iter().filter(|e|e.directory).count(),tab.entries.iter().filter(|e|!e.directory).count())})?;
        doc.text("job-progress",self.progress.map(|n|format!("Transferred {n} bytes…")).unwrap_or_default())?;
        doc.visible("progress-popup",self.progress.is_some())?;
        doc.text("progress-popup-copy",self.progress.map(|n|format!("Transferred {n} bytes")).unwrap_or_default())?;
        doc.visible("context-popup",self.context.is_some() && !self.chooser_loading && self.chooser.is_none())?;
        self.project_chooser(doc)?;
        let selected_path=self.selected();
        let mut visible_rows=0;
        let many=self.model.tab().selected_paths.len()>1;
        for item in MENU_ITEMS {
            let enabled=match item {
                "terminal"|"newtab"|"newfolder"|"bookmark"=>true,
                "paste"=>self.model.clipboard.is_some(),
                "extract"=>self.context_on_entry && !many && selected_path.as_deref().is_some_and(is_archive),
                "compress"|"copy"|"cut"|"trash"|"duplicate"=>self.context_on_entry && selected_path.is_some(),
                "open"|"openwith"|"rename"|"properties"=>self.context_on_entry && !many && selected_path.is_some(),
                _=>self.context_on_entry && selected_path.is_some(),
            };
            doc.visible(&format!("menu-{item}"),enabled)?;
            if enabled {visible_rows+=1;}
        }
        if let Some((x,y))=self.context {
            // Shared desktop context-menu metrics; hidden actions consume no space.
            let metrics=flamewm_ui_core::context_menu::MenuMetrics::canonical();
            let height=visible_rows as f32*metrics.row_height as f32+2.0*metrics.padding as f32;
            doc.position("context-popup",x.min(1060.0-210.0).max(0.0),y.min(720.0-height).max(0.0))?;
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
    std::thread::Builder::new().name("flamewm-explorer-io".into()).spawn(move||worker(jobs,result_tx,writer)).map_err(|e|e.to_string())?;
    let app=Rc::new(RefCell::new(ExplorerApp::new(directory,tx,scan_tx,chooser_tx)));
    app.borrow_mut().theme=initial_theme();
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
        UiWindowConfig{width:1060,height:720,x:80,y:65,title:"FlameWM Explorer".into(),identity:None},
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
        assert_eq!(app.folder_at(230.0,TOOLBAR_HEIGHT+COLUMNS_HEIGHT+1.0),Some(dir));
    }
}
