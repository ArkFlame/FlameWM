use super::super::*;

impl ExplorerApp {
    pub(crate) fn new(initial:PathBuf,tx:Sender<WorkerJob>,scan_tx:Sender<WorkerJob>,chooser_tx:Sender<ChooserJob>) -> Self {
        let home=home_dir();
        let trash_base=std::env::var_os("XDG_DATA_HOME").map_or_else(||home.join(".local/share"), PathBuf::from).join("Trash");
        let mut app=Self {model:Explorer::new(initial),tx,scan_tx,chooser_tx,chooser:None,thumbs:RefCell::new(Thumbnails::default()),thumb_tx:None,thumb_bound:RefCell::new([None;ROW_SLOTS]),chooser_loading:false,chooser_request:0,places:Vec::new(),context:None,edit:None,field:flamewm_explorer::TextField::default(),rename_target:None,theme:ExplorerTheme::default(),window:(WINDOW_WIDTH as f32,WINDOW_HEIGHT as f32),last_click:None,progress:None,context_on_entry:false,rubber:None,activity:None,busy:false,row_drag:None,drop_folder:None,file_drag_out:None,
            ark_extracts:Vec::new(),cancel:CancellationToken::new(),menu_overflow:false,context_scroll:0,glide:RowGlide::default(),scroll_origin:0,scroll_ticking:false,list_dirty:false,resize_gate:ResizeGate::default(),resize_flush_scheduled:false,widths:Widths::default(),show_type:false,col_drag:None,scroll_drag:None,column_sort:ColumnSort::default(),typed:String::new(),typed_at:None,
            dialog:None,ark:archive::find_ark().is_some(),search:None,search_id:0,search_text:String::new(),search_rows:None,watch:None,watched:None,device_watch:None,context_place:None,progress_total:0,trash_base,unmounted:Vec::new(),events:None,system_spent:None,remote:RemoteFs::new(RemoteConfig::default()),remote_cancel:None,remote_recent:prefs::load_recent(&prefs::config_dir())};
        app.reload_places();
        app
    }

    /// Built-in and bookmark places, plus the Trash folder just before mounted devices.
    pub(crate) fn reload_places(&mut self) {
        let home=home_dir();
        let mut list=places(&home,&self.unmounted);
        // Trash sits at the end of the Places group, as in the mock.
        let trash=Place{name:"Trash".into(),path:self.trash_base.join("files"),removable:false,group:PlaceGroup::Places,icon:"trash",device:None};
        let at=list.iter().rposition(|p|p.group==PlaceGroup::Places).map_or(0,|i|i+1);
        list.insert(at,trash);
        self.places=list;
    }

    /// Runs `job` on its own thread and wakes the UI with its result. Used for lsblk and udisks2, which can block.
    pub(crate) fn spawn_background(&self,job:impl FnOnce()->WorkerResult+Send+'static) {
        let Some((tx,wake))=self.events.as_ref() else {return};
        let (tx,Ok(mut wake))=(tx.clone(),wake.try_clone()) else {return};
        std::thread::spawn(move||{if tx.send(job()).is_ok() {let _=wake.write_all(&[1]);}});
    }

    /// Lists unmounted devices on a worker. Remounting remembered drives is the session's job, not the explorer's.
    pub(crate) fn refresh_devices(&self) {
        self.spawn_background(move||WorkerResult::Devices(bookmarks::unmounted_devices()));
    }

    /// Starts the live device signal: udev block events and mount-table changes reload the sidebar.
    pub(crate) fn watch_devices(&mut self) {
        self.device_watch=self.events.as_ref().and_then(|(tx,wake)|{
            let (tx,wake)=(tx.clone(),wake.try_clone().ok()?);
            DeviceWatch::spawn(move||{if tx.send(WorkerResult::DevicesChanged).is_ok() {let _=(&wake).write_all(&[1]);}}).ok()
        });
    }
}
