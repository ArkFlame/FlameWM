use super::super::*;

impl ExplorerApp {
    pub(crate) fn context_place_path(&self)->Option<PathBuf> {
        self.context_place.and_then(|i|self.places.get(i)).map(|p|p.path.clone())
    }

    pub(crate) fn is_bookmark(&self,index:usize)->bool {
        self.places.get(index).is_some_and(|p|p.group==PlaceGroup::Pinned && !p.removable && p.path!=self.trash_base.join("files"))
    }

    /// Edits the GTK bookmarks file atomically; the sidebar is rebuilt from the saved file.
    pub(crate) fn mutate_bookmark(&mut self,path:&Path,edit:impl FnOnce(&mut Vec<bookmarks::Bookmark>,usize)->bool) {
        let config=std::env::var_os("XDG_CONFIG_HOME").map_or_else(||home_dir().join(".config"), PathBuf::from);
        let file=bookmarks::bookmarks_file(&config);
        let mut items=bookmarks::load_bookmarks(&file);
        let Some(index)=items.iter().position(|b|b.path==path) else {self.model.status="Bookmark not found".into();return;};
        if !edit(&mut items,index) {return;}
        match bookmarks::save_bookmarks(&file,&items) {
            Ok(())=>{self.reload_places();self.model.status="Places updated".into();},
            Err(error)=>self.model.status=format!("Places not saved: {error}"),
        }
    }

    /// Mount, unmount or eject a removable device through udisks2 on a worker thread; the outcome is shown in the status strip.
    /// An eject here is a direct user action, so it carries the explicit confirmation the eject gate requires
    /// (`places::eject_device_in`); the drive is still refused while another of its partitions is mounted.
    pub(crate) fn device_action(&mut self,verb:&'static str,device:String,label:String) {
        self.model.status=format!("{} {label}…",match verb {"mount"=>"Mounting","unmount"=>"Unmounting",_=>"Ejecting"});
        self.spawn_background(move||{
            // A user unmount or eject is an explicit choice: no automatic remount until the user mounts it again.
            if verb!="mount" {bookmarks::set_remembered(&device,false);}
            let done=if verb=="eject" {bookmarks::eject_device_confirmed(&device)} else {bookmarks::udisks_action(verb,&device)};
            if done.is_ok() && verb=="mount" {bookmarks::set_remembered(&device,true);}
            WorkerResult::DeviceDone(match done {
                Ok(_)=>Ok(format!("{} {label}",match verb {"mount"=>"Mounted","unmount"=>"Unmounted",_=>"Ejected"})),
                Err(error)=>Err(format!("{verb} {label} failed: {error}")),
            })
        });
    }

    /// Opens a place, or mounts a removable device that is present but not mounted.
    pub(crate) fn activate_place(&mut self,index:usize) {
        let Some(place)=self.places.get(index).cloned() else {return};
        if place.mounted() {self.navigate(place.path);}
        else if let Some(device)=place.device {self.device_action("mount",device,place.name);}
    }

    /// Sidebar button at the right of a device row: eject when mounted, mount when not.
    pub(crate) fn place_button(&mut self,index:usize) {
        let Some(place)=self.places.get(index).cloned() else {return};
        let Some(device)=place.device.clone() else {return};
        if place.mounted() {self.device_action("eject",device,place.name);} else {self.device_action("mount",device,place.name);}
    }
}
