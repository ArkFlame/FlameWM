use super::*;
use flamewm_api::workspace::{WORKSPACE_ROWS, grid_position, workspace_columns};

/// `_NET_DESKTOP_GEOMETRY`: the whole two-row grid in pixels (columns x WORKSPACE_ROWS cells).
pub(super) fn desktop_geometry_payload(count: usize, screen: Rect) -> [u32; 2] {
    [
        (workspace_columns(count) as u32).saturating_mul(screen.width),
        (WORKSPACE_ROWS as u32).saturating_mul(screen.height),
    ]
}

/// `_NET_DESKTOP_VIEWPORT`: the cell origin of every desktop in index order. An index outside the
/// grid (only possible for an empty topology) publishes the origin rather than a bogus cell.
pub(super) fn desktop_viewport_payload(count: usize, screen: Rect) -> Vec<u32> {
    (0..count)
        .flat_map(|index| match grid_position(index, count) {
            Some(cell) => [
                (cell.column as u32).saturating_mul(screen.width),
                (cell.row as u32).saturating_mul(screen.height),
            ],
            None => [0, 0],
        })
        .collect()
}

impl<'a, C: Connection> Wm<'a, C> {
    fn set_property32(
        &self,
        window: Window,
        property: Atom,
        ty: AtomEnum,
        data: &[u32],
    ) -> Result<(), ReplyError> {
        self.conn
            .change_property32(PropMode::REPLACE, window, property, ty, data)?;
        Ok(())
    }

    pub(super) fn reconcile_work_area(&mut self) -> Result<(), ReplyError> {
        self.snap_preview.hide();
        let ids = self.clients.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let (mode, outer, minimized) = match self.clients.get(&id) {
                Some(state) => (state.placement.mode, state.outer, state.minimized),
                None => continue,
            };
            if minimized {
                continue;
            }
            let next = rehome_outer(mode, outer, &self.output_work_areas, self.work_area());
            if next != outer {
                if let Some(state) = self.clients.get_mut(&id) {
                    state.set_outer(next);
                }
                self.apply_frame_geometry(id)?;
                self.publish_window_state(id)?;
            }
        }
        self.mark_work_area();
        self.mark_windows();
        Ok(())
    }

    pub(super) fn publish_desktop_state(&self) -> Result<(), ReplyError> {
        let root = self.screen().root;
        self.set_property32(
            root,
            self.atoms.net_supported,
            AtomEnum::ATOM,
            &self.atoms.supported(),
        )?;
        self.set_property32(
            root,
            self.atoms.net_supporting_wm_check,
            AtomEnum::WINDOW,
            &[self.support_window],
        )?;
        self.set_property32(
            self.support_window,
            self.atoms.net_supporting_wm_check,
            AtomEnum::WINDOW,
            &[self.support_window],
        )?;
        self.conn.change_property8(
            PropMode::REPLACE,
            self.support_window,
            self.atoms.net_wm_name,
            self.atoms.utf8_string,
            b"FlameWM",
        )?;
        self.publish_workspace_metadata()?;
        self.publish_current_workspace()?;
        self.publish_client_list()?;
        self.publish_active()?;
        Ok(())
    }

    pub(super) fn publish_current_workspace(&self) -> Result<(), ReplyError> {
        let _guard = flamewm_profiler::start("wm.workspace.switch.ewmh_current");
        workspace_counters().ewmh_current.increment();
        self.set_property32(
            self.screen().root,
            self.atoms.net_current_desktop,
            AtomEnum::CARDINAL,
            &[self.current_workspace as u32],
        )?;
        Ok(())
    }

    pub(super) fn publish_workspace_metadata(&self) -> Result<(), ReplyError> {
        let root = self.screen().root;
        let work = self.work_area();
        self.set_property32(
            root,
            self.atoms.net_number_of_desktops,
            AtomEnum::CARDINAL,
            &[self.workspace_count as u32],
        )?;
        // _NET_CURRENT_DESKTOP owned by publish_current_workspace only.
        // The desktop strip is the product two-row grid: one screen-sized cell per desktop,
        // laid out in columns x WORKSPACE_ROWS, so external pagers see the same topology.
        self.set_property32(
            root,
            self.atoms.net_desktop_geometry,
            AtomEnum::CARDINAL,
            &desktop_geometry_payload(self.workspace_count, self.screen_rect),
        )?;
        let viewport = desktop_viewport_payload(self.workspace_count, self.screen_rect);
        let mut workareas = Vec::with_capacity(self.workspace_count * 4);
        let names = desktop_names_payload(self.workspace_count);
        for _ in 0..self.workspace_count {
            workareas.extend_from_slice(&[work.x as u32, work.y as u32, work.width, work.height]);
        }
        self.set_property32(
            root,
            self.atoms.net_desktop_viewport,
            AtomEnum::CARDINAL,
            &viewport,
        )?;
        self.set_property32(
            root,
            self.atoms.net_workarea,
            AtomEnum::CARDINAL,
            &workareas,
        )?;
        self.conn.change_property8(
            PropMode::REPLACE,
            root,
            self.atoms.net_desktop_names,
            self.atoms.utf8_string,
            &names,
        )?;
        self.set_property32(
            root,
            self.atoms.net_showing_desktop,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        Ok(())
    }

    pub(super) fn publish_client_list(&self) -> Result<(), ReplyError> {
        workspace_counters().query_tree.increment();
        // The root tree is the server's authoritative bottom-to-top order.  HashMap iteration
        // cannot represent stacking and made _NET_CLIENT_LIST_STACKING observably false.
        let tree = self.conn.query_tree(self.screen().root)?.reply()?;
        let windows = tree
            .children
            .iter()
            .filter_map(|frame| self.frame_to_client.get(frame).copied())
            .collect::<Vec<_>>();
        let client_list = self
            .managed_order
            .iter()
            .copied()
            .filter(|window| self.clients.contains_key(window))
            .collect::<Vec<_>>();
        let root = self.screen().root;
        self.set_property32(
            root,
            self.atoms.net_client_list,
            AtomEnum::WINDOW,
            &client_list,
        )?;
        self.set_property32(
            root,
            self.atoms.net_client_list_stacking,
            AtomEnum::WINDOW,
            &windows,
        )?;
        Ok(())
    }

    pub(super) fn publish_active(&self) -> Result<(), ReplyError> {
        let root = self.screen().root;
        self.set_property32(
            root,
            self.atoms.net_active_window,
            AtomEnum::WINDOW,
            &[self.active.unwrap_or(0)],
        )?;
        Ok(())
    }
}
