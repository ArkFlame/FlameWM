use super::super::*;

impl ExplorerApp {
    /// Sets `path` as the desktop and lock-screen wallpaper through Flame Control, off the UI thread.
    pub(crate) fn set_as_wallpaper(&mut self,path:PathBuf) {
        use flamewm_integrations_linux::media_store::{import_for_setting,MediaKind};
        use flamewm_settings_core::{SettingsClient,ControlTransport};
        struct Bus(flamewm_control_dbus::ControlClient);
        impl ControlTransport for Bus {
            fn call(&mut self,r:flamewm_control_core::ControlRequest)->Result<flamewm_control_core::ControlResponse,flamewm_control_core::ControlError> {self.0.call(&r)}
        }
        self.model.status="Setting wallpaper…".into();
        std::thread::spawn(move||{
            let run=||->Result<(),String>{
                let desktop=import_for_setting(MediaKind::DesktopWallpaper,&path)?;
                let lock=import_for_setting(MediaKind::LockWallpaper,&path)?;
                let bus=flamewm_control_dbus::ControlClient::connect(flamewm_dbus_reactor::BusKind::Session).map_err(|e|e.to_string())?;
                let mut c=SettingsClient::new(Bus(bus));
                // A fresh client has no revision; load the live snapshot or the write is refused as stale.
                c.refresh_settings().map_err(|e|format!("{e:?}"))?;
                c.set_wallpaper(desktop).map_err(|e|format!("{e:?}"))?;
                for (k,v) in [("wallpaperMode","custom".to_owned()),("lockBackgroundImage",lock),("lockBackgroundMode","custom".to_owned())] {
                    c.apply_setting(k,flamewm_api::settings::SettingValue::Text(v)).map_err(|e|format!("{e:?}"))?;
                }
                Ok(())
            };
            if let Err(e)=run() {eprintln!("flamewm-explorer: set as wallpaper failed: {e}");}
        });
    }

    /// Current preferences, as persisted. Every setting writes the whole file so none can drop another.
    pub(crate) fn current_prefs(&self)->prefs::Prefs {
        prefs::Prefs{show_hidden:self.model.hidden,show_type:self.show_type,col_modified:self.widths.modified as u32,col_size:self.widths.size as u32,col_kind:self.widths.kind as u32}
    }

    pub(crate) fn save_prefs(&mut self) {
        if let Err(error)=prefs::save(&prefs::config_dir(),&self.current_prefs()) {
            self.model.status=format!("Could not save Explorer settings: {error}");
        }
    }

    /// Show or hide dot files, persisting the choice for the next Explorer window.
    pub(crate) fn toggle_hidden(&mut self) {
        self.model.hidden = !self.model.hidden;
        self.save_prefs();
        self.scan();
    }

    /// Shows or hides the Type column (menu option), persisting the choice.
    pub(crate) fn toggle_show_type(&mut self) {
        self.show_type = !self.show_type;
        self.save_prefs();
    }

    pub(crate) fn request_open_with(&mut self, path:PathBuf) {
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

    pub(crate) fn dismiss_chooser(&mut self) {
        self.chooser_request=self.chooser_request.wrapping_add(1);
        self.chooser_loading=false;
        self.chooser=None;
    }

    pub(crate) fn launch_chooser_choice(&mut self, always:bool) {
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

    pub(crate) fn chooser_action(&mut self, action:&str) {
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

    /// Opens the configured terminal (`FlameWM` Terminal by default) in `directory`. Only `FlameWM` Terminal takes
    /// `--working-directory`; other emulators get the directory as their working directory and no command.
    pub(crate) fn spawn_terminal(&mut self,directory:PathBuf) {
        let program=flamewm_applications::terminal::terminal_program();
        let mut argv=flamewm_applications::terminal::terminal_command(&program,&[],Some(&directory));
        if argv.len()>1 && matches!(argv.last().map(String::as_str),Some("-e"|"--")) {argv.pop();}
        self.model.status=match Command::new(&argv[0]).args(&argv[1..]).current_dir(&directory).spawn() {
            Ok(_)=>"Opened terminal".into(),
            Err(e)=>format!("Terminal failed: {e}"),
        };
    }

    pub(crate) fn spawn_ark(&mut self, args:&[&str], path:&Path) {
        match Command::new("ark").args(args).arg(path).spawn() {
            Ok(_)=>self.model.status="Opened Ark".into(),
            Err(e)=>self.model.status=format!("Ark unavailable: {e}"),
        }
    }

    pub(crate) fn compress_selected(&mut self) {
        let paths=self.selected_files();
        if paths.is_empty() {return;}
        // Ark requires either a filename or --autofilename; without it the dialog only shows an error.
        match Command::new("ark").args(["--add","--autofilename","zip"]).args(&paths).spawn() {
            Ok(_)=>self.model.status="Opened Ark compression dialog".into(),
            Err(e)=>self.model.status=format!("Ark unavailable: {e}"),
        }
    }

    pub(crate) fn action_menu(&mut self,cmd:&str) {
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
            "setwallpaper"=>if let Some(path)=selected {self.set_as_wallpaper(path)},
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

    pub(crate) fn open_properties(&mut self,path:PathBuf) {
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

    /// Applies a permission string entered in the Properties address field.
    pub(crate) fn apply_mode(&mut self,text:&str) {
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
}
