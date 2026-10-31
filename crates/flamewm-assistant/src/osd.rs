//! Optional voice on-screen display: a small floating character that shows what the assistant
//! is doing. Owned by the assistant app; only the voice flow drives it.
//!
//! The state machine and animation are pure so they can be tested without a display. The host
//! owns the surface: create it from `ui/assistant/osd.html` on the first visible state, call
//! [`paint`] on each [`Osd::tick`] while visible, hide it when [`OsdFrame::visible`] is false.
//! Hidden has no timer and no surface.

use std::time::Duration;

use flamewm_ui_x11::UiDocumentAccess;

/// Frame interval while visible. About 10 frames per second; the breathing is driven by this.
pub const FRAME_MS: u64 = 100;
/// Breathing cycle length in frames (1.2 s at `FRAME_MS`).
pub const BREATH_FRAMES: u32 = 12;
/// Character diameter at the trough and the amplitude added at the crest.
pub const BASE_DIAMETER: u32 = 56;
pub const BREATH_AMPLITUDE: u32 = 4;

/// OSD surface size in px. Placement on screen is the host's job.
pub const WIDTH: u32 = 260;
pub const HEIGHT: u32 = 140;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsdState {
    Hidden,
    /// Character centred; Esc cancels.
    Listening,
    /// Character moved to the top-left corner; bubble beside it.
    Working,
    /// Click stops speech.
    Speaking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsdInput {
    Escape,
    Click,
}

/// Actions the voice flow must perform after an OSD input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsdEvent {
    CancelListening,
    StopSpeaking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Osd {
    state: OsdState,
    frame: u32,
}

impl Default for Osd {
    fn default() -> Self {
        Self::new()
    }
}

impl Osd {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: OsdState::Hidden,
            frame: 0,
        }
    }

    #[must_use]
    pub const fn state(&self) -> OsdState {
        self.state
    }

    /// Show `state`. `Hidden` is the same as [`Osd::hide`].
    pub fn show(&mut self, state: OsdState) {
        if state == OsdState::Hidden {
            self.hide();
        } else {
            self.state = state;
        }
    }

    pub fn hide(&mut self) {
        self.state = OsdState::Hidden;
        self.frame = 0;
    }

    /// True only while visible: the host runs the frame timer only then.
    #[must_use]
    pub const fn wants_timer(&self) -> bool {
        !matches!(self.state, OsdState::Hidden)
    }

    /// Route Esc or a click. Returns the action the voice flow must perform.
    pub fn input(&mut self, input: OsdInput) -> Option<OsdEvent> {
        match (self.state, input) {
            (OsdState::Listening, OsdInput::Escape) => {
                self.hide();
                Some(OsdEvent::CancelListening)
            }
            (OsdState::Speaking, OsdInput::Click) => {
                self.hide();
                Some(OsdEvent::StopSpeaking)
            }
            _ => None,
        }
    }

    /// Advance one animation frame. Returns true when the surface needs a repaint.
    pub fn tick(&mut self) -> bool {
        if !self.wants_timer() {
            return false;
        }
        self.frame = self.frame.wrapping_add(1) % BREATH_FRAMES;
        true
    }

    #[must_use]
    pub const fn frame_interval() -> Duration {
        Duration::from_millis(FRAME_MS)
    }

    #[must_use]
    pub fn view(&self) -> OsdFrame {
        let diameter = breath_diameter(self.frame);
        let (cx, cy, bubble) = match self.state {
            OsdState::Hidden | OsdState::Listening => ((WIDTH / 2) as i32, 40, (35, 80)),
            OsdState::Working | OsdState::Speaking => (28, 28, (56, 18)),
        };
        let eye = eye_size(diameter);
        let eye_dx = (diameter / 5) as i32;
        let (text, hint) = match self.state {
            OsdState::Hidden => ("", ""),
            OsdState::Listening => ("I'm listening...", "Esc cancels"),
            OsdState::Working => ("Working...", ""),
            OsdState::Speaking => ("Speaking...", "Click to stop"),
        };
        OsdFrame {
            visible: self.wants_timer(),
            character: Rect {
                left: cx - diameter as i32 / 2,
                top: cy - diameter as i32 / 2,
                size: diameter,
            },
            eyes: [
                Rect {
                    left: cx - eye_dx - eye as i32 / 2,
                    top: cy - eye as i32 / 2,
                    size: eye,
                },
                Rect {
                    left: cx + eye_dx - eye as i32 / 2,
                    top: cy - eye as i32 / 2,
                    size: eye,
                },
            ],
            bubble_left: bubble.0,
            bubble_top: bubble.1,
            text,
            hint,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub size: u32,
}

/// Everything the host needs to paint one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsdFrame {
    pub visible: bool,
    pub character: Rect,
    pub eyes: [Rect; 2],
    pub bubble_left: i32,
    pub bubble_top: i32,
    pub text: &'static str,
    pub hint: &'static str,
}

/// Triangle wave over one breathing cycle: `BASE` at frame 0, `BASE + AMPLITUDE` at mid-cycle.
#[must_use]
pub fn breath_diameter(frame: u32) -> u32 {
    let half = BREATH_FRAMES / 2;
    let phase = frame % BREATH_FRAMES;
    let t = if phase <= half {
        phase
    } else {
        BREATH_FRAMES - phase
    };
    BASE_DIAMETER + BREATH_AMPLITUDE * t / half
}

fn eye_size(diameter: u32) -> u32 {
    (diameter / 8).max(5)
}

/// Write one frame into the OSD document. The surface is shown or hidden by the host.
pub fn paint(frame: &OsdFrame, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
    let c = frame.character;
    doc.size("character", c.size as f32, c.size as f32)?;
    doc.position("character", c.left as f32, c.top as f32)?;
    for (id, eye) in [("eye-left", frame.eyes[0]), ("eye-right", frame.eyes[1])] {
        doc.size(id, eye.size as f32, eye.size as f32)?;
        doc.position(id, eye.left as f32, eye.top as f32)?;
    }
    doc.position("bubble", frame.bubble_left as f32, frame.bubble_top as f32)?;
    doc.text("osd-text", frame.text.to_owned())?;
    doc.text(
        "osd-hint",
        if frame.hint.is_empty() {
            "\u{200b}".to_owned()
        } else {
            frame.hint.to_owned()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_hidden_without_timer() {
        let osd = Osd::new();
        assert_eq!(osd.state(), OsdState::Hidden);
        assert!(!osd.wants_timer());
        assert!(!osd.view().visible);
    }

    #[test]
    fn show_and_hide_drive_the_timer() {
        let mut osd = Osd::new();
        osd.show(OsdState::Listening);
        assert_eq!(osd.state(), OsdState::Listening);
        assert!(osd.wants_timer());
        osd.show(OsdState::Hidden);
        assert_eq!(osd.state(), OsdState::Hidden);
        assert!(!osd.wants_timer());
    }

    #[test]
    fn tick_does_nothing_while_hidden() {
        let mut osd = Osd::new();
        assert!(!osd.tick());
        osd.show(OsdState::Working);
        assert!(osd.tick());
    }

    #[test]
    fn escape_cancels_only_listening() {
        let mut osd = Osd::new();
        osd.show(OsdState::Working);
        assert_eq!(osd.input(OsdInput::Escape), None);
        assert_eq!(osd.state(), OsdState::Working);
        osd.show(OsdState::Listening);
        assert_eq!(osd.input(OsdInput::Escape), Some(OsdEvent::CancelListening));
        assert_eq!(osd.state(), OsdState::Hidden);
    }

    #[test]
    fn click_stops_speech_only_while_speaking() {
        let mut osd = Osd::new();
        osd.show(OsdState::Listening);
        assert_eq!(osd.input(OsdInput::Click), None);
        osd.show(OsdState::Speaking);
        assert_eq!(osd.input(OsdInput::Click), Some(OsdEvent::StopSpeaking));
        assert_eq!(osd.state(), OsdState::Hidden);
    }

    #[test]
    fn breath_is_a_bounded_triangle_wave() {
        assert_eq!(breath_diameter(0), BASE_DIAMETER);
        assert_eq!(
            breath_diameter(BREATH_FRAMES / 2),
            BASE_DIAMETER + BREATH_AMPLITUDE
        );
        assert_eq!(breath_diameter(BREATH_FRAMES), BASE_DIAMETER);
        let half = BREATH_FRAMES / 2;
        let rising: Vec<u32> = (0..=half).map(breath_diameter).collect();
        assert!(rising.windows(2).all(|w| w[0] <= w[1]));
        for frame in 0..BREATH_FRAMES * 3 {
            let d = breath_diameter(frame);
            assert!((BASE_DIAMETER..=BASE_DIAMETER + BREATH_AMPLITUDE).contains(&d));
        }
    }

    #[test]
    fn tick_wraps_the_breath_cycle() {
        let mut osd = Osd::new();
        osd.show(OsdState::Listening);
        for _ in 0..BREATH_FRAMES {
            osd.tick();
        }
        assert_eq!(osd.view().character.size, BASE_DIAMETER);
    }

    #[test]
    fn listening_is_centred_and_working_moves_to_top_left() {
        let mut osd = Osd::new();
        osd.show(OsdState::Listening);
        let listening = osd.view();
        assert_eq!(
            listening.character.left + listening.character.size as i32 / 2,
            (WIDTH / 2) as i32
        );
        assert_eq!(listening.text, "I'm listening...");
        osd.show(OsdState::Working);
        let working = osd.view();
        assert!(working.character.left < listening.character.left);
        assert!(working.character.top < listening.character.top);
        assert_eq!(working.text, "Working...");
    }

    #[test]
    fn eyes_stay_inside_the_character() {
        let mut osd = Osd::new();
        osd.show(OsdState::Listening);
        for _ in 0..BREATH_FRAMES {
            let v = osd.view();
            for eye in v.eyes {
                assert!(eye.left >= v.character.left);
                assert!(eye.top >= v.character.top);
                assert!(eye.left + eye.size as i32 <= v.character.left + v.character.size as i32);
            }
            osd.tick();
        }
    }
}
