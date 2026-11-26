use crate::{UiEvent, WidgetId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toggle {
    pub checked: bool,
    pub enabled: bool,
}
impl Toggle {
    #[must_use]
    pub const fn new(checked: bool) -> Self {
        Self {
            checked,
            enabled: true,
        }
    }
    pub fn set_checked(&mut self, checked: bool) -> bool {
        if self.checked == checked {
            return false;
        }
        self.checked = checked;
        true
    }
    pub fn toggle(&mut self) -> bool {
        self.set_checked(!self.checked)
    }
    #[must_use]
    pub fn event(&self, widget: impl Into<WidgetId>) -> UiEvent {
        UiEvent::Toggle {
            widget: widget.into(),
            checked: self.checked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_changes_once_per_state_transition() {
        let mut toggle = Toggle::new(false);
        assert!(toggle.toggle());
        assert!(!toggle.set_checked(true));
        assert!(toggle.checked);
    }
}
