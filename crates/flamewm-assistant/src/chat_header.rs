//! Header subtitle for the chat window. A pure text rule, so the window code only forwards it.

/// Words the working status rotates through.
const VERBS: [&str; 12] = [
    "Thinking",
    "Scrambling",
    "Pondering",
    "Brewing",
    "Conjuring",
    "Mulling",
    "Noodling",
    "Untangling",
    "Cooking",
    "Wrangling",
    "Daydreaming",
    "Crunching",
];
/// Animation steps per word: two full dot cycles.
const STEPS_PER_VERB: u32 = 8;

/// Working status for animation step `phase`: the word, then dots `.` `..` `...` and none, repeating.
#[must_use]
pub fn working_text(phase: u32) -> String {
    let verb = VERBS[(phase / STEPS_PER_VERB) as usize % VERBS.len()];
    let dots = (phase % 4) as usize;
    format!("{verb}{}", &"..."[..dots])
}

/// Subtitle under the window title: the working status during a turn, otherwise "Assistant".
/// Nothing is probed at open, so there is no online or offline state to show.
#[must_use]
pub fn subtitle(working: Option<u32>) -> String {
    match working {
        Some(phase) => working_text(phase),
        None => "Assistant".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_is_assistant_when_idle_and_working_text_during_a_turn() {
        assert_eq!(subtitle(None), "Assistant");
        assert_eq!(subtitle(Some(0)), "Thinking");
    }

    #[test]
    fn working_text_cycles_dots_then_rotates_words() {
        let dots: Vec<String> = (0..5).map(working_text).collect();
        assert_eq!(
            dots,
            [
                "Thinking",
                "Thinking.",
                "Thinking..",
                "Thinking...",
                "Thinking"
            ]
        );
        assert_eq!(working_text(STEPS_PER_VERB), "Scrambling");
        assert_eq!(
            working_text(STEPS_PER_VERB * VERBS.len() as u32),
            "Thinking"
        );
    }
}
