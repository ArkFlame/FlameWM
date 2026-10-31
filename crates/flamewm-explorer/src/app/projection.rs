use super::super::*;

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

    pub(crate) fn project(&self, doc:&mut impl UiDocumentAccess)->Result<(),String>{
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
        self.project_rubber(doc)?;
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
        let mut shown:Vec<&str>=Vec::new();
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
                "setwallpaper"=>self.context_on_entry && !many && selected_path.as_deref().is_some_and(|p|!remote::is_remote(p) && p.is_file() && thumbs::thumb_format(p).is_some_and(crate::thumbs::ThumbFormat::is_image)),
                "opencontaining"=>searching && self.context_on_entry && !many && selected_path.is_some(),
                "restore"=>in_trash && self.context_on_entry && selected_path.is_some(),
                "hidden"|"showtype"|"connect"=>self.menu_overflow,
                _=>false,
            }};
            doc.visible(&format!("menu-{item}"),enabled)?;
            if enabled {shown.push(item);}
        }
        if let Some((x,y))=self.context {
            // Shared desktop context-menu metrics; hidden actions consume no space.
            let metrics=flamewm_ui_core::context_menu::MenuMetrics::canonical();
            let row_h=metrics.row_height as f32;
            let pad=metrics.padding as f32;
            // Rows that fit the window; a taller menu shows a window-sized slice that the wheel moves through.
            let fit=(((self.window.1-2.0*pad)/row_h).floor().max(0.0) as usize).min(shown.len());
            let first=self.context_scroll.min(shown.len()-fit);
            for (index,item) in shown.iter().enumerate() {doc.visible(&format!("menu-{item}"),index>=first && index<first+fit)?;}
            let height=fit as f32*row_h+2.0*pad;
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
    pub(crate) fn project_rows(&self, doc:&mut impl UiDocumentAccess, full:bool)->Result<(),String> {
        let tab=self.model.tab();
        let kind_shown=self.show_type;
        let rows=self.visible_rows();
        let origin=self.origin(rows);
        let top=list_top(self.model.tabs.len()>1);
        let bottom=self.window.1-STATUS_STRIP_HEIGHT;
        let glide=self.glide.offset(Instant::now());
        // One snapshot of the source for all slots, same sequence as `entry_at`; slot k maps to position k-1+origin.
        let first=origin.saturating_sub(1);
        let mut window: [Option<&Entry>; ROW_SLOTS] = [None; ROW_SLOTS];
        match self.search_rows.as_ref() {
            Some(results)=>{
                for (slot, entry) in results.iter().skip(first).take(ROW_SLOTS).enumerate() { window[slot]=Some(entry); }
            },
            None=>{
                for (slot, entry) in tab.visible().skip(first).take(ROW_SLOTS).enumerate() { window[slot]=Some(entry); }
            },
        }
        for slot in 0..ROW_SLOTS {
            let row=format!("row-{slot}");
            let index=flamewm_explorer::row_slot_index(origin,slot);
            let y=top+(slot as f32-1.0+glide)*ROW_HEIGHT;
            let entry=if slot<=rows+1 {usize::try_from(index).ok().and_then(|i|i.checked_sub(first)).and_then(|p|window.get(p).copied().flatten())} else {None};
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
                doc.background(&row,if self.drop_folder.as_deref()==Some(entry.path.as_path()) || tab.selected_paths.contains(&entry.path) {
                    self.theme.selected()
                }
                    else if index%2==0{self.theme.clear()}else{self.theme.row_alternate()})?;
            } else {doc.visible(&row,false)?;}
        }
        Ok(())
    }

    pub(crate) fn project_row_icon(&self, doc:&mut impl UiDocumentAccess, slot:usize, entry:&Entry)->Result<(),String> {
        let icon=view::file_icon(entry);
        let thumbnail=self.row_thumbnail(entry);
        match row_icon_choice(entry,thumbnail.as_ref()) {
            RowIcon::Thumbnail(id)=>{
                let Some((_,pixels))=thumbnail else {return self.show_type_icon(doc,slot,icon)};
                // The thumbnail draws in the same square as the type glyph. The element is sized to
                // ROW_ICON_BOX explicitly, which overrides the stylesheet's larger `.row-thumb` rule:
                // the name cell never jumps when a thumbnail arrives, and the 24px rule can never
                // squeeze the 18px glyph out of its cell.
                let thumb_id=format!("ico-{slot}-thumb");
                doc.size(&thumb_id,ROW_ICON_BOX,ROW_ICON_BOX)?;
                let mut bound=self.thumb_bound.borrow_mut();
                if bound[slot]!=Some(id) {
                    // A document that rejects the raster must not blank the row (or abort the whole
                    // projection, which would leave every later slot with a stale icon): fall back to
                    // the embedded type icon for this slot instead.
                    let bound_ok=doc.image_rgba8_shared(&thumb_id,format!("explorer-thumb-{id}"),THUMB_SIZE,THUMB_SIZE,pixels).is_ok();
                    bound[slot]=bound_ok.then_some(id);
                    if !bound_ok {drop(bound);return self.show_type_icon(doc,slot,icon);}
                }
                for kind in ROW_ICONS {doc.visible(&format!("ico-{slot}-{kind}"),false)?;}
                doc.visible(&thumb_id,true)?;
            }
            RowIcon::Type(icon)=>self.show_type_icon(doc,slot,icon)?,
        }
        Ok(())
    }

    /// Shows one embedded `row-*.svg` family icon and hides the thumbnail. The family name always
    /// comes from `view::file_icon`, whose values are the compiled `ROW_ICONS`. The glyph is sized
    /// explicitly so it stays visible even if the stylesheet's `.row-ico` box changes.
    pub(crate) fn show_type_icon(&self, doc:&mut impl UiDocumentAccess, slot:usize, icon:&'static str)->Result<(),String> {
        self.thumb_bound.borrow_mut()[slot]=None;
        doc.visible(&format!("ico-{slot}-thumb"),false)?;
        for kind in ROW_ICONS {
            let shown=kind==icon;
            doc.visible(&format!("ico-{slot}-{kind}"),shown)?;
            if shown {doc.size(&format!("ico-{slot}-{kind}"),ROW_ICON_BOX,ROW_ICON_BOX)?;}
        }
        Ok(())
    }

    /// Cached thumbnail for an image row. A cache miss queues one decode and shows the type icon meanwhile.
    pub(crate) fn row_thumbnail(&self, entry:&Entry)->Option<(u64,Arc<Vec<u8>>)> {
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
                            // A rejected raster hides the slot instead of aborting the projection: a failed bind must
                            // never leave the dialog (or the rows below it) unprojected.
                            let source=format!("explorer-app-icon-{}",choice.entry.id().as_str());
                            let bound=doc.image_rgba8_shared(&icon_id,source,icon.width,icon.height,Arc::clone(&icon.pixels)).is_ok();
                            doc.visible(&icon_id,bound)?;
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
            doc.text("openwith-page",String::new())?;
        }
        Ok(())
    }
}
