use super::super::*;

impl ExplorerApp {
    /// Folder drops (row drags and external drops) confirm first, with the same validator and dialog as paste.
    pub(crate) fn confirm_drop(&mut self,sources:Vec<PathBuf>,directory:PathBuf,force_copy:bool) {
        if self.refuse_if_busy() {return;}
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
    pub(crate) fn confirm_remote_copy(&mut self,sources:Vec<PathBuf>,directory:PathBuf) {
        if sources.is_empty() {self.model.status="Nothing to copy".into();return;}
        let name=match sources.as_slice() {
            [only]=>only.file_name().unwrap_or(only.as_os_str()).to_string_lossy().into_owned(),
            many=>format!("{} items",many.len()),
        };
        let location=remote::url_of(&directory).map_or_else(||ops::display_location(&directory,&home_dir()), |url|remote::display_text(&url));
        let title=if sources.len()==1 {"Copy file?"} else {"Copy items?"};
        self.dialog=Some(Dialog::Confirm{title:title.into(),lines:vec![name,location],pending:Pending::RemoteCopy{sources,directory}});
    }

    /// The files a row drag carries: the whole selection when the pressed row is part of it.
    pub(crate) fn dragged_sources(&self,source:&Path)->Vec<PathBuf> {
        if self.model.tab().selected_paths.contains(source) {self.selected_files()} else {vec![source.to_path_buf()]}
    }

    /// Files dropped from another application copy into the current folder. XDND only offers copy here.
    pub(crate) fn ark_drop(&mut self,service:String,path:String) {
        if remote::is_remote(self.model.tab().path()) {
            self.model.status="Drops into remote folders are not available yet".into();
            return;
        }
        let destination=self.model.tab().path().to_path_buf();
        self.ark_extracts.push(flamewm_explorer::dndext::spawn_ark_extract(service,path,destination));
        self.model.status="Extracting from Ark…".into();
    }

    /// External drop: a folder row under the pointer receives the files; anywhere else means the current folder.
    pub(crate) fn external_drop(&mut self,paths:Vec<PathBuf>,x:f32,y:f32) {
        let destination=self.folder_at(x,y).unwrap_or_else(||self.model.tab().path().to_path_buf());
        if remote::is_remote(&destination) {self.confirm_remote_copy(paths,destination);}
        else {self.confirm_drop(paths,destination,true);}
    }

    pub(crate) fn finish_row_drag(&mut self,e:&UiActionEvent)->bool {
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

    pub(crate) fn start_transfer(&mut self,sources:Vec<PathBuf>,move_it:bool) {
        let directory=self.model.tab().path().to_path_buf();
        self.start_transfer_to(sources,directory,move_it);
    }

    /// Duplicate: copy each source beside itself as `name (copy)`; no conflict prompt, since the name is always fresh.
    pub(crate) fn start_duplicate(&mut self,sources:Vec<PathBuf>) {
        if self.refuse_if_busy() {return;}
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

    pub(crate) fn start_transfer_to(&mut self,sources:Vec<PathBuf>,directory:PathBuf,move_it:bool) {
        if self.refuse_if_busy() {return;}
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

    pub(crate) fn send_transfer(&mut self,sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,conflict:Conflict) {
        self.progress=Some(0);
        self.activity=None;
        self.model.status=format!("{} {} item(s)…",if move_it{"Moving"}else{"Copying"},sources.len());
        if self.tx.send(WorkerJob::Transfer{sources,directory,move_it,conflict,cancel:self.cancel.clone()}).is_err() {
            self.progress=None;
            self.model.status="Transfer worker unavailable".into();
        } else {self.busy=true;}
    }

    /// Queues a remote operation on the file worker. Shares the busy flag with local file work.
    pub(crate) fn send_remote(&mut self,op:RemoteOp,status:&str)->bool {
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
    pub(crate) fn start_remote_copy(&mut self,sources:Vec<PathBuf>,directory:PathBuf) {
        if sources.is_empty() {self.model.status="Nothing to copy".into();return;}
        let count=sources.len();
        let op=RemoteOp::Copy{sources,directory,cancel:self.cancel.clone()};
        if self.send_remote(op,&format!("Copying {count} item(s)…")) {
            self.progress=Some(0);
            self.activity=None;
        }
    }

    /// Publishes a file list to the system CLIPBOARD on a detached thread, so the UI never waits on it.
    pub(crate) fn publish_system_clipboard(&mut self,paths:Vec<PathBuf>,cut:bool) {
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
    pub(crate) fn request_paste(&mut self) {
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

    pub(crate) fn apply_paste(&mut self,resolved:Option<(Vec<PathBuf>,bool)>) {
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

    pub(crate) fn confirm_delete(&mut self) {
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
    pub(crate) fn confirm_remote_delete(&mut self,paths:Vec<PathBuf>) {
        if paths.is_empty() {return;}
        let name=match paths.as_slice() {
            [only]=>only.file_name().unwrap_or(only.as_os_str()).to_string_lossy().into_owned(),
            many=>format!("{} items",many.len()),
        };
        let location=paths.first().and_then(|p|remote::url_of(p)).and_then(|url|url.parent()).map(|parent|remote::display_text(&parent)).unwrap_or_default();
        self.dialog=Some(Dialog::Confirm{title:"Delete permanently?".into(),lines:vec![name,location],pending:Pending::RemoteDelete(paths)});
    }

    pub(crate) fn request_trash(&mut self,paths:Vec<PathBuf>) {
        self.context=None;
        match ops::validate(ops::Kind::Trash,&source_facts(&paths),None,true) {
            Verdict::Run(paths)=>{let location=parent_dir(&paths);self.confirm_items("Move to trash?",&paths,&location,Pending::Trash(paths.clone()));},
            Verdict::NoOp=>{},
            Verdict::Refuse(message)=>self.model.status=message.into(),
        }
    }

    /// The one reusable confirmation: title, item name (or "N items"), and the location shortened to `~`.
    pub(crate) fn confirm_items(&mut self,title:&str,paths:&[PathBuf],location:&Path,pending:Pending) {
        let name=match paths {
            [only]=>only.file_name().unwrap_or(only.as_os_str()).to_string_lossy().into_owned(),
            many=>format!("{} items",many.len()),
        };
        let lines=vec![name,ops::display_location(location,&home_dir())];
        self.dialog=Some(Dialog::Confirm{title:title.into(),lines,pending});
    }

    /// Button slots: primary, secondary, tertiary, cancel. Each dialog kind decides what they mean.
    pub(crate) fn dialog_action(&mut self,slot:&str) {
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

    pub(crate) fn run_pending(&mut self,pending:Pending) {
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
                if self.refuse_if_busy() {return;}
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
}
