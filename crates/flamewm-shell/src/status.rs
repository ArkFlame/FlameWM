use flamewm_api::system::SystemSnapshot;
use flamewm_shell_core::StatusViews;

#[must_use]
pub fn from_snapshot(system: &SystemSnapshot) -> StatusViews {
    StatusViews::from_snapshot(system)
}
