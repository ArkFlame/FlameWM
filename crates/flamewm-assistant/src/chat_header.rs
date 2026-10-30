//! Header subtitle and empty-state copy for the chat window. Pure text rules, so the window
//! code only forwards the result.

use flamewm_assistant_core::AssistantState;

pub const GREETING: &str = "What do you need help with?";

/// Short Hermes status line shown under the window title.
#[must_use]
pub fn subtitle(state: &AssistantState, working: bool, model_only: bool) -> &'static str {
    match state {
        AssistantState::Ready { .. } if working => "Thinking...",
        AssistantState::Ready { .. } if model_only => "Model only, no tools",
        AssistantState::Ready { .. } => "Connected",
        AssistantState::Starting { .. } => "Starting...",
        AssistantState::Stopped { .. } => "Not running",
        AssistantState::Error { .. } => "Unavailable",
        AssistantState::NotInstalled => "Not installed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_follows_the_lifecycle_state() {
        let ready = AssistantState::Ready { version: String::new(), url: String::new() };
        assert_eq!(subtitle(&ready, false, false), "Connected");
        assert_eq!(subtitle(&ready, true, false), "Thinking...");
        assert_eq!(subtitle(&ready, false, true), "Model only, no tools");
        assert_eq!(subtitle(&AssistantState::NotInstalled, false, false), "Not installed");
        assert_eq!(subtitle(&AssistantState::Stopped { version: String::new() }, false, false), "Not running");
        assert_eq!(
            subtitle(&AssistantState::Starting { version: String::new(), attempts: 0 }, false, false),
            "Starting..."
        );
        assert_eq!(
            subtitle(&AssistantState::Error { message: "x".into(), version: None }, false, false),
            "Unavailable"
        );
    }
}
