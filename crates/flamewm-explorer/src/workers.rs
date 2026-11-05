//! Background worker loops for the explorer: thumbnails, directory scans and file jobs.
//! Pure move out of main.rs; each loop talks to the UI through the shared wake socket.
use super::*;

/// Decodes queued image keys one at a time, oldest first; the UI reads results from the cache, never waits on this thread.
pub(crate) fn thumb_worker(queue:Arc<thumbs::ThumbQueue>, result:Sender<WorkerResult>, mut wake:UnixStream) {
    loop {
        let key=queue.pop();
        let pixels=thumbs::render_thumbnail(&key);
        send_wake(&mut wake,&result,WorkerResult::Thumbnail{key,pixels});
    }
}
pub(crate) fn send_wake(wake:&mut UnixStream, result:&Sender<WorkerResult>, message:WorkerResult) {
    if result.send(message).is_ok() {let _=wake.write_all(&[1]);}
}

/// Where the reporter's accepted snapshots go: the UI result channel plus the wake byte.
pub(crate) fn snapshot_sink(result:&Sender<WorkerResult>, wake:&UnixStream)->impl FnMut(Snapshot)+Send+'static {
    let (result,wake)=(result.clone(),wake.try_clone().ok());
    move |snapshot:Snapshot|{
        if result.send(WorkerResult::Progress(Box::new(snapshot))).is_ok() {
            if let Some(mut wake)=wake.as_ref() {let _=wake.write_all(&[1]);}
        }
    }
}

/// Counts the batch, then runs `body` with the terminal snapshot published from
/// the outcome. Counting and every byte/entry/item update happen on this worker
/// thread; the UI only ever sees snapshots.
pub(crate) fn run_transfer(
    reporter:&mut progress::Reporter, cancel:&CancellationToken,
    body:impl FnOnce(&mut progress::Reporter)->BatchReport,
)->BatchReport {
    reporter.announce(JobState::Counting);
    let report=if cancel.is_cancelled() {
        BatchReport{cancelled:true,..BatchReport::default()}
    } else {
        reporter.announce(JobState::Running);
        body(reporter)
    };
    reporter.finish(if report.cancelled {JobState::Cancelled}
        else if report.failed>0 {JobState::PartiallyFailed} else {JobState::Completed});
    report
}
/// Streams each listing as sorted batches (first rows paint early), then sends the complete sorted listing.
/// A newer queued scan abandons the current one between batches: its rows are never sent as final.
pub(crate) fn scan_worker(rx:Receiver<WorkerJob>, result:Sender<WorkerResult>, mut wake:UnixStream) {
    let mut next:Option<WorkerJob>=None;
    loop {
        let mut job=match next.take() {
            Some(job)=>job,
            None=>match rx.recv() {Ok(job)=>job,Err(_)=>return},
        };
        // Obsolete queued navigation requests must never block the latest scan.
        while let Ok(newer)=rx.try_recv() { job=newer; }
        let WorkerJob::Scan{generation,path,hidden,sort}=job else {continue};
        let mut all=Vec::new();
        let mut first=true;
        let streamed=list_directory_streamed(&path,hidden,|mut batch|{
            if let Ok(newer)=rx.try_recv() { next=Some(newer); return false; }
            sort_listing(&mut batch,sort);
            all.extend(batch.iter().cloned());
            send_wake(&mut wake,&result,WorkerResult::ScanBatch{generation,path:path.clone(),first,entries:batch});
            first=false;
            true
        });
        match streamed {
            Ok(false)=>continue,
            Ok(true)=>{
                sort_listing(&mut all,sort);
                send_wake(&mut wake,&result,WorkerResult::Scanned{generation,entries:Ok(all),path});
            },
            Err(error)=>send_wake(&mut wake,&result,WorkerResult::Scanned{generation,entries:Err(error),path}),
        }
    }
}
/// Moves one path to the user's Trash and returns the restorable journal entry.
pub(crate) fn trash_one(path:&Path)->std::io::Result<Undo> {
    let home=home_dir();
    let data=std::env::var_os("XDG_DATA_HOME").map_or_else(||home.join(".local/share"), PathBuf::from);
    let uid=fs::read_to_string("/proc/self/status").ok()
        .and_then(|s|flamewm_desktop_core::trash::current_uid_from_proc(&s))
        .ok_or_else(||std::io::Error::new(std::io::ErrorKind::PermissionDenied,"cannot determine current user"))?;
    Trash::move_to_trash(path,&home,&data,uid)
        .map(|item|Undo::Trashed{original:item.original,stored:item.stored,info:item.info})
        .map_err(|e|std::io::Error::other(e.to_string()))
}
/// Undo (`undoing`) or redo the newest entry of the bounded history and report it in status text.
pub(crate) fn history_step(undoing:bool,trash:&mut dyn FnMut(&Path)->std::io::Result<Undo>)->Result<String,String> {
    let mut journal=jobs::journal().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if undoing {
        let Some(entry)=journal.take_last() else {return Err("Nothing to undo".to_owned());};
        match jobs::undo_with(&entry,trash) {
            Ok(())=>{let text=format!("Undid: {}",jobs::describe(&entry));journal.push_redo(entry);Ok(text)},
            Err(error)=>{journal.restore(entry);Err(format!("Undo failed: {error}"))},
        }
    } else {
        let Some(entry)=journal.take_redo() else {return Err("Nothing to redo".to_owned());};
        match jobs::redo_with(&entry,trash) {
            Ok(next)=>{let text=format!("Redid: {}",jobs::describe(&next));journal.push_undo(next);Ok(text)},
            Err(error)=>{journal.push_redo(entry);Err(format!("Redo failed: {error}"))},
        }
    }
}
pub(crate) fn worker(rx:Receiver<WorkerJob>, result:Sender<WorkerResult>, mut wake:UnixStream) {
    for job in rx {
        match job {
            WorkerJob::Scan{..}=>{},
            WorkerJob::Transfer{sources,directory,move_it,conflict,cancel}=>{
                let mut reporter=progress::Reporter::connected(JobId::next(),cancel.clone(),snapshot_sink(&result,&wake));
                let totals=jobs::count_sources(&sources,&cancel);
                reporter.set_totals(totals.bytes,totals.items);
                let mut report=run_transfer(&mut reporter,&cancel,|reporter|{
                    jobs::transfer_batch_with(&sources,&directory,move_it,conflict,&cancel,reporter)
                });
                jobs::record(report.undo.drain(..));
                send_wake(&mut wake,&result,WorkerResult::BatchCompleted {
                    report,action:if move_it {"Moved"}else{"Copied"},
                    cut_sources:move_it.then_some(sources),
                });
            }
            WorkerJob::Drop{sources,directory,force_copy,cancel}=>{
                let mut reporter=progress::Reporter::connected(JobId::next(),cancel.clone(),snapshot_sink(&result,&wake));
                let totals=jobs::count_sources(&sources,&cancel);
                reporter.set_totals(totals.bytes,totals.items);
                let mut report=run_transfer(&mut reporter,&cancel,|reporter|{
                    jobs::transfer_drop_batch_with(&sources,&directory,force_copy,&cancel,reporter)
                });
                jobs::record(report.undo.drain(..));
                send_wake(&mut wake,&result,WorkerResult::BatchCompleted {
                    report,action:"Transferred",cut_sources:None,
                });
            }
            WorkerJob::Trash{paths}=>{
                let mut report=BatchReport::default();
                for path in &paths {
                    match trash_one(path) {
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
                send_wake(&mut wake,&result,WorkerResult::Completed(done.map(|()|"Renamed".to_owned()).map_err(|e|e.to_string())));
            }
            WorkerJob::Undo=>{
                let done=history_step(true,&mut trash_one);
                send_wake(&mut wake,&result,WorkerResult::Completed(done));
            }
            WorkerJob::Redo=>{
                let done=history_step(false,&mut trash_one);
                send_wake(&mut wake,&result,WorkerResult::Completed(done));
            }
            WorkerJob::Remote{fs,op}=>remote_job(&fs,op,&mut wake,&result),
        }
    }
}
