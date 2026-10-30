//! Per-client controller bookkeeping and the controller geometry cache.
use super::*;

/// Controller geometry derived from live registry and output state.
pub(super) struct ControllerGeometry {
    pub(super) client: Window,
    pub(super) children: [Window; 12],
    pub(super) regions: [Option<FrameRegion>; 12],
    pub(super) region_map: Rc<HashMap<u32, FrameRegion>>,
    pub(super) outputs: Rc<Vec<RootRect>>,
}

/// Reuses the last `ControllerGeometry` while its inputs are unchanged. The key is the
/// live inputs themselves (no revision counter to forget to bump), so a hit is exact.
#[derive(Default)]
pub(super) struct ControllerGeometryCache {
    pub(super) entry: Option<ControllerGeometry>,
    #[cfg(test)]
    pub(super) rebuilds: usize,
}

impl ControllerGeometryCache {
    pub(super) fn fetch(
        &mut self,
        client: Window,
        children: [Window; 12],
        regions: [Option<FrameRegion>; 12],
        outputs: impl Iterator<Item = RootRect> + Clone,
    ) -> &ControllerGeometry {
        let fresh = self.entry.as_ref().is_some_and(|entry| {
            entry.client == client
                && entry.children == children
                && entry.regions == regions
                && entry.outputs.iter().copied().eq(outputs.clone())
        });
        if !fresh {
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
            let region_map = children
                .iter()
                .zip(regions.iter())
                .filter_map(|(xid, region)| region.map(|region| (*xid, region)))
                .collect();
            self.entry = Some(ControllerGeometry {
                client,
                children,
                regions,
                region_map: Rc::new(region_map),
                outputs: Rc::new(outputs.collect()),
            });
        }
        self.entry.as_ref().expect("entry populated above")
    }
}

impl<'a, C: Connection> Wm<'a, C> {
    pub(super) fn work_root(&self) -> RootRect {
        to_root_rect(self.work_area())
    }

    pub(super) fn controller_for(&mut self, client: Window) -> Option<ControllerState> {
        let state = self.clients.get(&client)?;
        let res = self.registry.lookup_by_client(client)?;
        let session = self.session_of(client);
        let children = res.children();
        let regions =
            children.map(|xid| self.registry.lookup_by_xid(xid).map(|target| target.region));
        let outputs = self
            .output_work_areas
            .iter()
            .map(|output| RootRect::from_legacy(output.geometry));
        let (region_map, outputs) = {
            let geometry = self
                .controller_geometry
                .fetch(client, children, regions, outputs);
            (
                Rc::clone(&geometry.region_map),
                Rc::clone(&geometry.outputs),
            )
        };
        let screen_rect = RootRect::new(
            0,
            0,
            i32::from(self.screen().width_in_pixels),
            i32::from(self.screen().height_in_pixels),
        );
        let last_title_click = self
            .sessions
            .get(&client)
            .and_then(|session| session.last_title_click);
        let mut controller = Some(
            interaction::build_controller(
                client,
                state.frame,
                state.placement,
                to_root_rect(self.work_area_for_rect(state.outer)),
                screen_rect,
                state.hints,
                self.extents_for(client),
                session,
                region_map,
            )
            .with_outputs(outputs)
            .with_contents_while_dragging(crate::live_settings::show_content_while_dragging())
            .with_window_margin(crate::live_settings::window_margin()),
        );
        if let Some(controller) = controller.as_mut() {
            controller.last_title_click = last_title_click;
        }
        controller
    }

    /// Output geometries in root coordinates: the bound for drags (windows may overflow the work area).
    pub(super) fn output_root_rects(&self) -> Vec<RootRect> {
        self.output_work_areas
            .iter()
            .map(|output| RootRect::from_legacy(output.geometry))
            .collect()
    }

    pub(super) fn store_controller(&mut self, client: Window, controller: &ControllerState) {
        let (sessions, clients) = (&mut self.sessions, &mut self.clients);
        interaction::store_controller(sessions, clients.get_mut(&client), client, controller);
        if let Some(entry) = sessions.get_mut(&client) {
            entry.last_title_click = controller.last_title_click;
        }
    }
}
