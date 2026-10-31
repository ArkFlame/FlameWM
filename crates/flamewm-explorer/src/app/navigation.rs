use super::super::*;

impl ExplorerApp {
    /// Reports and returns true when a file operation is already running; callers return early.
    pub(crate) fn refuse_if_busy(&mut self)->bool {
        if self.busy {self.model.status="File operation already in progress".into();}
        self.busy
    }

    /// The single selected folder entry, when the context menu is on an entry.
    pub(crate) fn selected_folder(&self)->Option<PathBuf> {
        if self.model.tab().selected_paths.len()!=1 {return None;}
        let path=self.selected()?;
        self.model.tab().entries.iter().any(|entry|entry.path==path && entry.directory).then_some(path)
    }

    /// Folder that Open Terminal Here and Add Folder to Places act on: a right-clicked folder, else the current folder.
    pub(crate) fn target_directory(&self)->PathBuf {
        if self.context_on_entry {if let Some(folder)=self.selected_folder() {return folder;}}
        self.model.tab().path().to_path_buf()
    }

    pub(crate) fn clear_search(&mut self) {
        self.search_rows=None;
        if let Some(search)=self.search.as_mut() {search.cancel();}
    }

    pub(crate) fn start_search(&mut self,query:String) {
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

    pub(crate) fn take_search_results(&mut self) {
        let Some(search)=self.search.as_mut() else {return};
        let events=search.poll();
        for event in events {
            match event {
                SearchEvent::Batch{hits,..}=>{
                    let rows=self.search_rows.get_or_insert_with(Vec::new);
                    rows.extend(hits.into_iter().map(|hit|Entry::new(hit.path,hit.name,hit.directory,false,hit.size,hit.modified)));
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
    pub(crate) fn origin(&self,rows:usize)->usize {
        match self.search_rows.as_ref() {
            Some(results)=>self.model.tab().scroll.min(results.len().saturating_sub(rows)),
            None=>self.model.tab().window_start(rows),
        }
    }

    pub(crate) fn entry_at(&self,index:isize)->Option<&Entry> {
        let index=usize::try_from(index).ok()?;
        match self.search_rows.as_ref() {
            Some(results)=>results.get(index),
            None=>self.model.tab().visible_nth(index),
        }
    }

    pub(crate) fn visible_entries(&self)->Vec<&Entry> {
        let rows=self.visible_rows();
        if let Some(results)=self.search_rows.as_ref() {
            let start=self.model.tab().scroll.min(results.len().saturating_sub(rows));
            return results.iter().skip(start).take(rows).collect();
        }
        self.model.tab().visible_window(rows)
    }

    /// Rows that fit between the column header and the status strip. Paint, hit-testing and keys all use this.
    pub(crate) fn visible_rows(&self)->usize {
        let bottom=self.window.1-STATUS_STRIP_HEIGHT;
        let fit=((bottom-list_top(self.model.tabs.len()>1))/ROW_HEIGHT).floor();
        fit.clamp(1.0,ROW_SLOTS as f32) as usize
    }

    pub(crate) fn sort_by(&mut self,column:Column) {
        self.column_sort=self.column_sort.click(column);
        view::sort_entries(&mut self.model.tab_mut().entries,self.column_sort);
    }

    /// Type-ahead moves the selection to the first visible name starting with the typed prefix.
    pub(crate) fn typeahead(&mut self,text:&str) {
        let now=Instant::now();
        if self.typed_at.is_none_or(|at|now.duration_since(at)>TYPEAHEAD_RESET) {self.typed.clear();}
        self.typed.push_str(text);
        self.typed_at=Some(now);
        let tab=self.model.tab();
        // One pass over the visible rows: per-index `visible_nth` rescans the list for every name.
        let names=tab.visible().map(|e|e.name.as_str()).collect::<Vec<_>>();
        let from=self.selected().as_ref().and_then(|p|tab.visible_index_of(p)).unwrap_or(0);
        if let Some(index)=view::typeahead_match(&names,&self.typed,from) {
            let path=tab.visible_nth(index).expect("index from typeahead").path.clone();
            let rows=self.visible_rows();
            let tab=self.model.tab_mut();
            tab.select_one(path);
            tab.reveal(index,rows);
        }
    }

    pub(crate) fn scan(&mut self) {
        self.row_drag=None;
        self.drop_folder=None;
        self.activity=None;
        self.request_listing();
    }

    /// Lists the current folder without clearing transient feedback (used by live refresh).
    pub(crate) fn request_listing(&mut self) {
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
    pub(crate) fn start_remote_listing(&mut self,generation:u64,url:RemoteUrl) {
        let cancel=RemoteCancel::new();
        self.remote_cancel=Some(cancel.clone());
        self.model.tab_mut().clear_entries();
        self.model.status="Loading…".into();
        let path=remote::to_path(&url);
        let events=self.remote.list_dir(&url,&cancel);
        let Some((tx,wake))=self.events.as_ref().and_then(|(tx,wake)|Some((tx.clone(),wake.try_clone().ok()?))) else {
            self.model.status="Remote listing needs the event loop".into();
            return;
        };
        std::thread::spawn(move||forward_listing(generation,path,url,events,tx,wake));
    }

    pub(crate) fn save_remote_recent(&mut self) {
        if let Err(error)=prefs::save_recent(&prefs::config_dir(),&self.remote_recent) {
            self.model.status=format!("Recent servers not saved: {error}");
        }
    }

    /// Downloads a remote file into the cache, then opens the local copy with the default application.
    pub(crate) fn download_and_open(&mut self,url:RemoteUrl) {
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

    pub(crate) fn navigate(&mut self,path:PathBuf) {
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
    pub(crate) fn row_entry(&self,slot:usize) -> Option<&Entry> {
        self.entry_at(flamewm_explorer::row_slot_index(self.origin(self.visible_rows()),slot))
    }

    pub(crate) fn chosen(&self,slot:usize) -> Option<&Entry> {
        self.visible_entries().get(slot).copied()
    }

    pub(crate) fn selected(&self) -> Option<PathBuf> {self.model.tab().selected.clone()}

    /// Drop coordinates belong to the Explorer surface. Only visible folder rows
    /// inside the file pane are eligible; no guessed external-window targets.
    pub(crate) fn folder_at(&self,x:f32,y:f32)->Option<PathBuf> {
        if !x.is_finite() || !y.is_finite() || !(SIDEBAR_WIDTH..self.window.0).contains(&x) {return None;}
        let top=list_top(self.model.tabs.len()>1);
        if y<top || y>=top+ROW_HEIGHT*self.visible_rows() as f32 {return None;}
        let slot=((y-top)/ROW_HEIGHT) as usize;
        self.chosen(slot).filter(|entry|entry.directory).map(|entry|entry.path.clone())
    }

    /// Unknown host key: fetch the key for the trust dialog. Failed sign-in: retry dialog. Others: status only.
    pub(crate) fn remote_blocked(&mut self,path:PathBuf,error:RemoteError) {
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

    /// Appends the user-confirmed key to `known_hosts` on a thread (the file write and the check never run on the UI thread).
    pub(crate) fn save_host_key(&mut self,url:RemoteUrl,prompt:HostKeyPrompt) {
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

    pub(crate) fn open(&mut self, path:&Path) {
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
        // Images go to the FlameWM Image Viewer when it is installed; otherwise the XDG path below applies.
        if flamewm_image_core::ImageSourceFormat::from_extension(path).is_some() {
            match Command::new("flamewm-imageviewer").arg(path).spawn() {
                Ok(_)=>return,
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
                Err(e)=>{self.model.status=format!("Image viewer failed: {e}");return;},
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

    pub(crate) fn tab_shows(&self,url:&RemoteUrl)->bool {
        remote::url_of(self.model.tab().path()).is_some_and(|tab|tab.to_string()==url.to_string())
    }

    pub(crate) fn selected_files(&self)->Vec<PathBuf> {self.model.tab().ordered_selection()}

    /// Ctrl+Z (`undoing`) and Ctrl+Shift+Z / Ctrl+Y redo, run on the file worker so the UI never blocks.
    pub(crate) fn history_last(&mut self,undoing:bool) {
        if self.refuse_if_busy() {return;}
        let job=if undoing {WorkerJob::Undo} else {WorkerJob::Redo};
        if self.tx.send(job).is_ok() {self.busy=true;self.model.status=if undoing {"Undoing…"} else {"Redoing…"}.into();}
        else {self.model.status="File worker unavailable".into();}
    }

    pub(crate) fn open_containing(&mut self,path:PathBuf) {
        let Some(parent)=path.parent().map(Path::to_path_buf) else {return};
        self.clear_search();
        self.navigate(parent);
        self.model.tab_mut().select_one(path);
    }
}
