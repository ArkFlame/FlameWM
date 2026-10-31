use super::super::*;

impl ExplorerApp {
    /// Starts a text edit. `select` marks a char range that the next typed character replaces.
    pub(crate) fn begin_edit(&mut self,mode:EditMode,text:String,select:Option<(usize,usize)>) {
        self.edit=Some(mode);
        match select {
            Some(range)=>self.field.set_selected(text,range),
            None=>self.field.set(text),
        }
    }

    /// Applies a finished text edit: address, search, permissions, place name or file rename.
    pub(crate) fn commit_edit(&mut self,edit:EditMode,content:&str) {
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
    pub(crate) fn commit_rename(&mut self,path:PathBuf,content:&str) {
        match flamewm_desktop_core::file_actions::validate_rename_name(content) {
            Ok(name)=>{
                if path.file_name().map(|n|n.to_string_lossy().into_owned())==Some(name.to_string()) {return;}
                if self.refuse_if_busy() {return;}
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
}
