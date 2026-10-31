use super::*;

impl ShellRuntime {
    pub fn dispatch(&mut self, action: &str) {
        self.dispatch_with_output(action, None);
    }

    /// Dispatch an input action with its already-resolved source output.
    /// The supplied target is captured into the control before another event
    /// can change panel selection. `None` is reserved for global actions,
    /// whose output is resolved by the shell's keyboard policy.
    pub fn dispatch_with_output(&mut self, action: &str, output: Option<flamewm_api::OutputId>) {
        self.dispatch_with_interaction(action, output, None);
    }

    pub fn dispatch_with_context(&mut self, action: &str, context: InteractionContext) {
        self.dispatch_with_interaction(action, Some(context.output.clone()), Some(context));
    }

    pub(super) fn dispatch_with_interaction(
        &mut self,
        action: &str,
        output: Option<flamewm_api::OutputId>,
        interaction: Option<InteractionContext>,
    ) {
        let control =
            match action {
                "start.toggle" => Some(ShellControl::ToggleStart {
                    output: output.clone(),
                    interaction: interaction.clone(),
                }),
                "start.close" => Some(ShellControl::CloseStart),
                value if value.starts_with("start.category.") => start::category_for_action(value)
                    .map(|category| ShellControl::SelectStartCategory {
                        category,
                        output: output.clone(),
                        interaction: interaction.clone(),
                    }),
                "media.open" | "media.toggle" | "volume.open" | "network.open" | "battery.open"
                | "clock.open" | "weather.open" => Some(ShellControl::OpenPopover {
                    name: action.split('.').next().unwrap_or(action).to_owned(),
                    output,
                    interaction,
                }),
                "popover.close" => Some(ShellControl::ClosePopovers),
                "session.lock" => Some(ShellControl::SessionAction(SessionAction::Lock)),
                "session.logout" => Some(ShellControl::SessionAction(SessionAction::Logout)),
                "session.suspend" => Some(ShellControl::SessionAction(SessionAction::Suspend)),
                "session.reboot" => Some(ShellControl::SessionAction(SessionAction::Reboot)),
                "session.shutdown" => Some(ShellControl::SessionAction(SessionAction::Shutdown)),
                value if value.starts_with("start.") => {
                    Some(ShellControl::Launch(value[6..].to_owned()))
                }
                _ => None,
            };
        if let Some(control) = control {
            self.controls.push_back(control);
        }
    }

    pub fn drain_controls(&mut self) -> impl Iterator<Item = ShellControl> + '_ {
        self.controls.drain(..)
    }

    /// True when a signal- or dispatch-queued control awaits the drain.
    #[must_use]
    pub fn has_pending_controls(&self) -> bool {
        !self.controls.is_empty()
    }

    /// Takes the pending Alt+Tab switcher snapshot, if one arrived since the last tick.
    pub fn take_switcher_frame(&mut self) -> Option<crate::switcher::SwitcherFrame> {
        self.switcher.take()
    }
}
