//! Start menu projection, selection, launch outcomes and slide/backdrop passes.

use super::*;

impl ShellLoop {
    /// Projects and presents the Start group on `output`. Returns `false` when
    /// the popup refused the open, leaving the caller's state untouched.
    pub(super) fn open_start_surface(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
        started: Instant,
    ) -> Result<bool, UiBackendError> {
        self.project_start(runtime)?;
        let _measure = shell_stage!("shell.start.open.measure");
        let _place = shell_stage!("shell.start.open.place");
        let _prepare = shell_stage!("shell.start.open.prepare");
        self.apply_start_backdrop(runtime);
        self.apply_start_slide(runtime);
        let opened = self.surfaces.open_start_group_for_output(runtime, output);
        let _present = shell_stage!("shell.start.open.present");
        shell_slow_note("start", started);
        match opened {
            Ok(()) => Ok(true),
            Err(error) if is_popup_refusal(&error) => Ok(false),
            Err(error) => Err(UiBackendError::Renderer(error)),
        }
    }

    pub(crate) fn project_start(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        // C07: project stage only. The caller owns the operation total, so
        // nested totals never double-own one operation.
        let _project = shell_stage!("shell.start.open.project");
        // This projection re-requests every visible icon under the current theme.
        self.start_icons_stale = false;
        let start_model = self.start_model.clone();
        let category = self.start_category;
        let query = self.start_query.clone();
        let session = self.shell.snapshot().session;
        self.ensure_loader();
        let loader = self.icon_loader.as_mut();
        // ONE unified document on the single Start surface: categories +
        // search + app rows + power rows project in one pass, so one
        // intrinsic union, one move_resize, one show/present follow.
        runtime.with_document(self.surfaces.start, |document| {
            async_projection::project_start_unified(
                document,
                &start_model,
                category,
                &query,
                &session,
                loader,
            )
        })?;
        self.project_start_error_line(runtime);
        self.project_start_selection(runtime)
    }

    /// Moves the keyboard selection through the visible Start rows (clamped, no wrap).
    pub(crate) fn move_start_selection(
        &mut self,
        runtime: &mut SurfaceRuntime,
        delta: isize,
    ) -> Result<(), UiBackendError> {
        let count = self.start_visible_count();
        if self.start_selection.step(delta, count) {
            self.project_start_selection(runtime)?;
        }
        Ok(())
    }

    /// Pointer over Start app row `slot` (0-based): hover replaces the keyboard highlight.
    /// Repaints only when the highlighted row actually changed.
    pub(crate) fn hover_start_slot(
        &mut self,
        runtime: &mut SurfaceRuntime,
        slot: usize,
    ) -> Result<(), UiBackendError> {
        let count = self.start_visible_count();
        if self.start_selection.hover(slot, count) {
            self.project_start_selection(runtime)?;
        }
        Ok(())
    }

    /// Clears the Start selection (pointer left the list or the list content changed).
    pub(crate) fn reset_start_selection(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        if self.start_selection.reset() {
            self.project_start_selection(runtime)?;
        }
        Ok(())
    }

    pub(super) fn start_visible_count(&self) -> usize {
        projection::start_visible_results(&self.start_model, self.start_category, &self.start_query)
            .len()
            .min(async_projection::START_APP_SLOT_COUNT)
    }

    pub(super) fn project_start_selection(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let highlighted = self
            .start_selection
            .highlighted()
            .filter(|index| *index < self.start_visible_count());
        runtime.with_document(self.surfaces.start, |document| {
            async_projection::project_start_highlight(document, highlighted)
        })
    }

    pub(crate) fn launch_start_slot(
        &mut self,
        runtime: &mut SurfaceRuntime,
        slot: usize,
    ) -> Result<(), UiBackendError> {
        let Some(app_id) = projection::start_application_for_slot(
            &self.start_model,
            self.start_category,
            &self.start_query,
            slot,
        ) else {
            return Ok(());
        };
        let app_name = projection::start_application_name_for_slot(
            &self.start_model,
            self.start_category,
            &self.start_query,
            slot,
        )
        .unwrap_or_else(|| "application".to_owned());
        self.start_error = None;
        self.project_start_error_line(runtime);
        let Some(lease) = self
            .popup_session
            .active()
            .filter(|lease| lease.kind == flamewm_shell_core::popup_session::PopupKind::Start)
            .cloned()
        else {
            return Ok(());
        };
        // The menu stays open until the launch result arrives: success closes
        // it in `apply_start_launch_outcomes`, failure keeps it open with the
        // Start error line. Submission itself never blocks.
        let invocation = flamewm_shell::control_actions::enqueue_start_launch(
            self.dispatcher.as_ref(),
            app_name,
            ControlRequest::LaunchApplication {
                app: app_id,
                options: flamewm_api::applications::ApplicationLaunchOptions::default(),
            },
        );
        self.active_start_launch = Some(StartLaunchContext {
            invocation,
            output: lease.output,
            popup_sequence: lease.sequence,
        });
        Ok(())
    }

    /// Apply Start launch results drained from the mutation result source.
    /// A failure sets the Start error line and keeps the menu open; a success
    /// clears the line and closes the menu that launched it.
    pub(super) fn apply_start_launch_outcomes(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        use flamewm_shell::control_actions::{start_error_after_launch, StartLaunchOutcome};
        let outcomes = flamewm_shell::control_actions::take_launch_outcomes();
        let Some(context) = self.active_start_launch.clone() else {
            return Ok(());
        };
        let Some(outcome) =
            flamewm_shell::control_actions::matching_launch_outcome(outcomes, context.invocation)
        else {
            return Ok(());
        };
        if !start_launch_session_matches(&context, self.start_open, self.popup_session.active()) {
            self.active_start_launch = None;
            return Ok(());
        }
        self.start_error = start_error_after_launch(&outcome);
        if matches!(outcome, StartLaunchOutcome::Started { .. }) {
            self.active_start_launch = None;
            return self.close_transients(runtime);
        }
        self.active_start_launch = None;
        self.project_start_error_line(runtime);
        runtime.redraw(self.surfaces.start)?;
        Ok(())
    }

    /// Project the Start error line into the Start surface document. Errors
    /// are nonfatal diagnostics so a projection fault never breaks the loop.
    pub(super) fn project_start_error_line(&mut self, runtime: &mut SurfaceRuntime) {
        let message = self.start_error.clone();
        let result = runtime.with_document(self.surfaces.start, |document| {
            projection::project_start_error(document, message.as_deref())
        });
        if let Err(error) = result {
            eprintln!("flamewm-shell: start error line projection failed: {error:?}");
        }
    }

    /// Applies the Start slide for the next show/hide. Derived from the cached
    /// settings and the current panel edge, so it runs before each open and
    /// after each settings projection. A rejected slide is nonfatal and
    /// falls back to the immediate show path.
    pub(super) fn apply_start_slide(&mut self, runtime: &mut SurfaceRuntime) {
        // Magic Lamp folds into the panel's Start button; without its layout the show stays immediate.
        let start_button = runtime
            .node_device_rect_by_id(
                self.surfaces.selected_panel(),
                flamewm_shell::popup_role::PopupRole::Start.source_node_id(),
            )
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
            .map(|rect| {
                (
                    rect.x as i32,
                    rect.y as i32,
                    rect.width.max(1.0) as u32,
                    rect.height.max(1.0) as u32,
                )
            });
        let slide = self.settings_cache.as_ref().and_then(|snapshot| {
            crate::entry::settings_projection::start_slide(
                snapshot,
                self.surfaces.panel_edge(),
                start_button,
            )
        });
        self.surfaces.set_start_motion(slide.is_some());
        if let Err(error) = runtime.set_slide(self.surfaces.start, slide) {
            eprintln!("flamewm-shell: start slide setup failed (nonfatal): {error:?}");
            let _ = runtime.set_slide(self.surfaces.start, None);
        }
        // Same Start preference and speed for task menu, media, and status popups. Magic Lamp
        // has no popup target, so the popup slide never takes the start_button path.
        let popup_slide = self.settings_cache.as_ref().and_then(|snapshot| {
            crate::entry::settings_projection::start_slide(
                snapshot,
                self.surfaces.panel_edge(),
                None,
            )
        });
        self.surfaces.set_popup_slide(popup_slide);
    }

    /// Installs the Start popup backdrop percents; the snapshot itself is captured on each first show.
    pub(super) fn apply_start_backdrop(&self, runtime: &mut SurfaceRuntime) {
        let Some(settings) = self.settings_cache.as_ref() else {
            return;
        };
        let percents = crate::entry::settings_projection::start_backdrop_percents(settings);
        if let Err(error) = runtime.set_popup_backdrop(self.surfaces.start, Some(percents)) {
            eprintln!("flamewm-shell: start backdrop failed (nonfatal): {error:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::production_source;
    #[test]
    fn monitor_scoped_start_toggle_transfers_only_when_the_target_output_changes() {
        let output_a = flamewm_api::OutputId::new("A");
        let output_b = flamewm_api::OutputId::new("B");
        assert_eq!(
            super::start_toggle_action(false, None, &output_a),
            super::StartToggleAction::Open,
        );
        assert_eq!(
            super::start_toggle_action(true, Some(&output_a), &output_a),
            super::StartToggleAction::Close,
        );
        assert_eq!(
            super::start_toggle_action(true, Some(&output_a), &output_b),
            super::StartToggleAction::Transfer,
        );
        assert_eq!(
            super::start_toggle_action(true, None, &output_b),
            super::StartToggleAction::Transfer,
        );
    }

    #[test]
    fn start_launch_result_cannot_cross_the_popup_session_origin() {
        use flamewm_shell_core::popup_session::{PopupKind, PopupLease};
        let context = super::StartLaunchContext {
            invocation: 8,
            output: flamewm_api::OutputId::new("A"),
            popup_sequence: 12,
        };
        let lease_a = PopupLease {
            output: flamewm_api::OutputId::new("A"),
            kind: PopupKind::Start,
            topology_generation: 3,
            panels_revision: 6,
            sequence: 12,
        };
        let lease_b = PopupLease {
            output: flamewm_api::OutputId::new("B"),
            sequence: 13,
            ..lease_a.clone()
        };
        assert!(super::start_launch_session_matches(
            &context,
            true,
            Some(&lease_a)
        ));
        assert!(!super::start_launch_session_matches(
            &context,
            true,
            Some(&lease_b)
        ));
        assert!(!super::start_launch_session_matches(
            &context,
            false,
            Some(&lease_a)
        ));
    }

    #[test]
    fn start_slide_pass_also_arms_popup_slide_without_start_button_target() {
        let source = production_source();
        let body = source
            .split_once("fn apply_start_slide(")
            .and_then(|(_, rest)| rest.split_once("\n    }\n"))
            .map(|(body, _)| body)
            .expect("apply_start_slide must exist");
        let body: String = body.split_whitespace().collect();
        assert!(
            body.contains("self.surfaces.set_popup_slide(popup_slide);"),
            "apply_start_slide must arm the popup slide on every Start-slide refresh"
        );
        assert!(
            body.contains(
                "crate::entry::settings_projection::start_slide(snapshot,self.surfaces.panel_edge(),None"
            ),
            "popup slide must use the shared start_slide owner with no Magic Lamp target"
        );
    }
}
