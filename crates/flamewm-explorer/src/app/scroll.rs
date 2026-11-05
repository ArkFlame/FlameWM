use super::super::*;

/// Starts one thread that runs `on_tick` once per request, at least `period` after the request.
/// Requests queue on the returned sender; the thread exits when the sender is dropped or `on_tick` returns false.
pub(crate) fn spawn_frame_ticker(period:Duration,mut on_tick:impl FnMut()->bool+Send+'static)->Sender<()> {
    let (requests,pending)=mpsc::channel::<()>();
    std::thread::spawn(move||{
        while pending.recv().is_ok() {
            std::thread::sleep(period);
            if !on_tick() {break;}
        }
    });
    requests
}

impl ExplorerApp {
    /// Tracks the window origin after every input or wake. A small step glides from the old position;
    /// a folder change or a jump larger than the window snaps.
    /// Settled client size from the WM. Returns true when the layout must be projected now. Sizes that
    /// arrive within `RESIZE_INTERVAL` of the last applied one are held; the trailing timer applies the last.
    pub(crate) fn resize(&mut self,width:u32,height:u32)->bool {
        let size=(width.max(1),height.max(1));
        match self.resize_gate.offer(size,Instant::now()) {
            Resize::Apply=>{self.apply_size(size);true},
            Resize::Defer(delay)=>{
                if !self.resize_flush_scheduled {
                    self.resize_flush_scheduled=true;
                    self.spawn_background(move||{std::thread::sleep(delay);WorkerResult::ResizeFlush});
                }
                false
            },
        }
    }

    pub(crate) fn flush_resize(&mut self) {
        self.resize_flush_scheduled=false;
        if let Some(size)=self.resize_gate.flush(Instant::now()) {self.apply_size(size);}
    }

    pub(crate) fn apply_size(&mut self,(width,height):(u32,u32)) {
        self.window=(width as f32,height as f32);
        self.sync_scroll();
    }

    pub(crate) fn sync_scroll(&mut self) {
        let rows=self.visible_rows();
        let origin=self.origin(rows);
        if origin!=self.scroll_origin {
            let step=origin as isize-self.scroll_origin as isize;
            self.glide.origin_moved(step,Instant::now(),rows);
            self.scroll_origin=origin;
            self.list_dirty=true;
        }
        if self.glide.is_animating(Instant::now()) {self.schedule_scroll_tick();}
    }

    pub(crate) fn schedule_scroll_tick(&mut self) {
        if self.scroll_ticking {return;}
        if self.scroll_timer.is_none() {
            let Some((tx,wake))=self.events.as_ref() else {return};
            let (tx,Ok(mut wake))=(tx.clone(),wake.try_clone()) else {return};
            self.scroll_timer=Some(spawn_frame_ticker(SCROLL_FRAME,move||{
                if tx.send(WorkerResult::ScrollTick).is_err() {return false;}
                let _=wake.write_all(&[1]);
                true
            }));
        }
        let Some(timer)=self.scroll_timer.as_ref() else {return};
        if timer.send(()).is_ok() {self.scroll_ticking=true;} else {self.scroll_timer=None;}
    }

    /// One frame of the glide. The frame that finds the glide finished marks the list dirty so its rest position is projected.
    pub(crate) fn step_scroll(&mut self) {
        self.scroll_ticking=false;
        if self.glide.is_animating(Instant::now()) {self.schedule_scroll_tick();} else {self.list_dirty=true;}
    }

    /// Top and height of the scrollbar track: the list area between the header and the status strip.
    pub(crate) fn scrollbar_track(&self)->(f32,f32) {
        let top=list_top(self.model.tabs.len()>1);
        (top,(self.window.1-STATUS_STRIP_HEIGHT-top).max(0.0))
    }

    /// Entries in the list, counting search results while a search is active.
    pub(crate) fn scroll_total(&self)->usize {
        match self.search_rows.as_ref() {
            Some(results)=>results.len(),
            None=>self.model.tab().filtered_count(),
        }
    }

    /// Column dividers move with the pointer in window coordinates. Each motion applies only its own delta.
    pub(crate) fn column_grip(&mut self,divider:Divider,e:&UiActionEvent) {
        match e.phase {
            UiActionPhase::Press if e.button==1 && e.inside => self.col_drag=Some((divider,e.x)),
            UiActionPhase::Motion => if let Some((active,last))=self.col_drag {
                let total=self.window.0-SIDEBAR_WIDTH;
                self.widths.drag_divider(active,e.x-last,total,self.show_type);
                self.col_drag=Some((active,e.x));
            },
            UiActionPhase::Release if self.col_drag.is_some() => {
                self.col_drag=None;
                self.save_prefs();
            },
            _=>{},
        }
    }

    /// Thumb drag moves the list by pixel ratio, snapping without glide so the rows track the pointer.
    /// Track clicks page one window towards the click.
    pub(crate) fn scrollbar_event(&mut self,action:&str,e:&UiActionEvent) {
        let (track_top,track_h)=self.scrollbar_track();
        let rows=self.visible_rows();
        let total=self.scroll_total();
        match e.phase {
            UiActionPhase::Press if e.button==1 && e.inside => {
                let origin=self.origin(rows);
                if action=="explorer.scroll.thumb" {
                    self.scroll_drag=Some((e.y,origin));
                } else if let Some((thumb_top,_))=scroll::thumb_geometry(track_h,total,rows,origin) {
                    let step=if e.y<track_top+thumb_top {-(rows as isize)} else {rows as isize};
                    self.set_origin((origin as isize+step).clamp(0,total.saturating_sub(rows) as isize) as usize,rows);
                }
            },
            UiActionPhase::Motion => if let Some((start_y,start_origin))=self.scroll_drag {
                let target=scroll::drag_origin(track_h,total,rows,start_origin,e.y-start_y);
                self.set_origin(target,rows);
            },
            UiActionPhase::Release => self.scroll_drag=None,
            _=>{},
        }
    }

    /// Sets the window origin without a glide; the drag and track paths follow the pointer directly.
    pub(crate) fn set_origin(&mut self,origin:usize,rows:usize) {
        self.model.tab_mut().scroll=origin;
        self.glide=RowGlide::default();
        self.scroll_origin=self.origin(rows);
    }
}

#[cfg(test)]
mod frame_ticker_tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Instant;

    #[test]
    fn one_tick_per_request_after_the_period_on_a_single_thread() {
        let (ticks_tx,ticks)=channel::<std::thread::ThreadId>();
        let started=Instant::now();
        let requests=spawn_frame_ticker(Duration::from_millis(20),move||{ticks_tx.send(std::thread::current().id()).is_ok()});
        for _ in 0..3 {
            requests.send(()).unwrap();
            let id=ticks.recv_timeout(Duration::from_secs(5)).expect("tick");
            assert_ne!(id,std::thread::current().id());
            assert!(started.elapsed()>=Duration::from_millis(20));
        }
        // No request, no tick.
        assert!(ticks.recv_timeout(Duration::from_millis(60)).is_err());
        // Same worker thread served every request.
        requests.send(()).unwrap();
        let again=ticks.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(again,std::thread::current().id());
    }

    #[test]
    fn dropping_the_sender_stops_the_thread() {
        let (ticks_tx,ticks)=channel::<()>();
        let requests=spawn_frame_ticker(Duration::from_millis(1),move||ticks_tx.send(()).is_ok());
        drop(requests);
        assert!(ticks.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
