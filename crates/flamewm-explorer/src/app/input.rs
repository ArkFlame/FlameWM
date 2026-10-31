use super::super::*;

impl ExplorerApp {
    /// Clamps a rubber-band point into the file list: right of the sidebar, below the header, above the status strip.
    pub(crate) fn clamp_to_list(&self,x:f32,y:f32)->(f32,f32) {
        let top=list_top(self.model.tabs.len()>1);
        let bottom=(self.window.1-STATUS_STRIP_HEIGHT).max(top);
        (x.clamp(SIDEBAR_WIDTH,self.window.0.max(SIDEBAR_WIDTH)),y.clamp(top,bottom))
    }

    pub(crate) fn event(&mut self,e:&UiActionEvent) {
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
        // An open context menu takes the wheel so rows that do not fit the window can be reached.
        if action=="scroll" && self.context.is_some() {
            let dy=e.text.as_deref().and_then(|t|t.split_once(',')).and_then(|(_,dy)|dy.parse::<f32>().ok()).unwrap_or(0.0);
            if dy>0.0 {self.context_scroll+=1;} else if dy<0.0 {self.context_scroll=self.context_scroll.saturating_sub(1);}
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
                self.context=Some((e.x,e.y));self.menu_overflow=false;self.context_scroll=0;
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
                    self.context=Some((e.x,e.y));self.menu_overflow=false;self.context_scroll=0;
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
                    self.context=Some((e.x,e.y));self.menu_overflow=false;self.context_scroll=0;
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
            "explorer.search"=>{if !matches!(self.edit,Some(EditMode::Search)){let text=self.model.tab().search.clone();self.begin_edit(EditMode::Search,text,None);}},
            "explorer.search.clear"=>{self.edit=None;self.clear_search();self.search_text.clear();self.model.status="Search cleared".into();},
            "explorer.address"=>{if !matches!(self.edit,Some(EditMode::Path)){let text=self.model.tab().path().to_string_lossy().into_owned();self.begin_edit(EditMode::Path,text,None);}},
            "explorer.tab.new"=>{if self.model.tabs.len()<5 {self.model.new_tab();self.scan();}},
            "explorer.tab.close"=>{self.model.close_tab();self.scan();},
            "explorer.sort.name"=>self.sort_by(Column::Name),
            "explorer.sort.size"=>self.sort_by(Column::Size),
            "explorer.sort.modified"=>self.sort_by(Column::Modified),
            "explorer.sort.kind"=>self.sort_by(Column::Type),
            "explorer.menu"=>{
                // The overflow button toggles: a second click on it closes the menu.
                if self.context.is_some() && self.menu_overflow {self.context=None;self.menu_overflow=false;}
                else {self.context_on_entry=false;self.menu_overflow=true;self.context_scroll=0;self.context=Some((self.window.0-8.0,53.0));}
            },
            "explorer.cancel"=>if self.busy {self.cancel.cancel();self.model.status="Cancelling…".into();},
            _=>{}
        }
    }

    pub(crate) fn key(&mut self,key:&UiKeyEvent) {
        self.key_inner(key);
        self.write_debug_field();
    }

    /// Debug-only read-back: `FLAMEWM_EXPLORER_DEBUG_FIELD`=<file> receives the active field's length and text after each key.
    pub(crate) fn write_debug_field(&self) {
        let Some(path)=std::env::var_os("FLAMEWM_EXPLORER_DEBUG_FIELD") else {return};
        let mode=match self.edit {Some(EditMode::Path)=>"path",Some(EditMode::Search)=>"search",Some(_)=>"other",None=>"none"};
        let _=std::fs::write(path,format!("{}\n{}\n{}",self.field.text.chars().count(),mode,self.field.text));
    }

    #[allow(clippy::suspicious_else_formatting)]
    pub(crate) fn key_inner(&mut self,key:&UiKeyEvent) {
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
        if let Some(edit)=self.edit.filter(|_|self.dialog.is_none()) {
            // Escape closes an open menu first and keeps the edit focused; a second Escape ends the edit.
            if name=="escape" && self.context.is_some() {self.context=None;self.menu_overflow=false;self.context_place=None;return;}
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

    pub(crate) fn step_selection(&mut self,delta:isize,extend:bool){
        let tab=self.model.tab();
        let count=tab.filtered_count();
        if count==0{return;}
        let current=self.selected().as_ref().and_then(|p|tab.visible_index_of(p)).unwrap_or(0);
        let next=(current as isize+delta).clamp(0,count as isize-1) as usize;
        self.select_index(next,extend);
    }

    /// Select the filtered row at `index` and scroll the window just enough to show it.
    pub(crate) fn select_index(&mut self,index:usize,extend:bool){
        let rows=self.visible_rows();
        let tab=self.model.tab();
        let Some(path)=tab.visible_nth(index).map(|entry|entry.path.clone()) else {return};
        let tab=self.model.tab_mut();
        if extend {tab.select_range_to(path)}else{tab.select_one(path)}
        tab.reveal(index,rows);
    }

    pub(crate) fn wake(&mut self,event:WorkerResult){
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
            WorkerResult::DevicesChanged=>{self.reload_places();self.refresh_devices();},
            WorkerResult::Devices(devices)=>{
                if self.unmounted!=devices {self.unmounted=devices;self.reload_places();}
            },
            WorkerResult::DeviceDone(outcome)=>{
                let message=outcome.unwrap_or_else(|error|error);
                self.model.status.clone_from(&message);
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
                self.model.status.clone_from(&message);
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
}
