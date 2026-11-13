use super::*;
use flamewm_ui_core::style::UiLayer;
use flamewm_ui_x11::{Overflow, RuntimeImage};

const TASK_MENU_HTML: &str = include_str!("../../../../../ui/shell/task-menu.html");

/// Records `(method, id)` for every write and never fails, so the set is exactly what projection asked for.
#[derive(Default)]
struct Recorder {
    writes: Vec<(&'static str, String)>,
}

impl Recorder {
    fn hit(&mut self, method: &'static str, id: &str) -> Result<(), String> {
        self.writes.push((method, id.to_owned()));
        Ok(())
    }
}

impl UiDocumentAccess for Recorder {
    fn text(&mut self, id: &str, _: String) -> Result<(), String> {
        self.hit("text", id)
    }
    fn image_rgb8(&mut self, id: &str, _: RuntimeImage) -> Result<(), String> {
        self.hit("image_rgb8", id)
    }
    fn image_rgba8(&mut self, id: &str, _: RuntimeImage) -> Result<(), String> {
        self.hit("image_rgba8", id)
    }
    fn visible(&mut self, id: &str, _: bool) -> Result<(), String> {
        self.hit("visible", id)
    }
    fn toggle(&mut self, track_id: &str, _: &str, _: bool) -> Result<(), String> {
        self.hit("toggle", track_id)
    }
    fn position(&mut self, id: &str, _: f32, _: f32) -> Result<(), String> {
        self.hit("position", id)
    }
    fn size(&mut self, id: &str, _: f32, _: f32) -> Result<(), String> {
        self.hit("size", id)
    }
    fn background(&mut self, id: &str, _: UiColor) -> Result<(), String> {
        self.hit("background", id)
    }
    fn border(&mut self, id: &str, _: UiColor) -> Result<(), String> {
        self.hit("border", id)
    }
    fn foreground(&mut self, id: &str, _: UiColor) -> Result<(), String> {
        self.hit("foreground", id)
    }
    fn background_clear(&mut self, id: &str) -> Result<(), String> {
        self.hit("background_clear", id)
    }
    fn attribute(&mut self, id: &str, _: &str, _: &str) -> Result<(), String> {
        self.hit("attribute", id)
    }
    fn border_clear(&mut self, id: &str) -> Result<(), String> {
        self.hit("border_clear", id)
    }
    fn foreground_clear(&mut self, id: &str) -> Result<(), String> {
        self.hit("foreground_clear", id)
    }
    fn font_size(&mut self, id: &str, _: f32) -> Result<(), String> {
        self.hit("font_size", id)
    }
    fn font_weight(&mut self, id: &str, _: u16) -> Result<(), String> {
        self.hit("font_weight", id)
    }
    fn overflow(&mut self, id: &str, _: Overflow, _: Overflow) -> Result<(), String> {
        self.hit("overflow", id)
    }
    fn layer(&mut self, id: &str, _: UiLayer) -> Result<(), String> {
        self.hit("layer", id)
    }
}

fn project_idle() -> Recorder {
    let mut recorder = Recorder::default();
    let snapshot = ShellSnapshot::default();
    let settings = SettingsSnapshot::new(0);
    project_context_menu(&mut recorder, &snapshot, &settings, None, &[], 0).expect("projection");
    recorder
}

#[test]
fn task_menu_projection_touches_only_existing_roots() {
    let recorder = project_idle();
    assert!(!recorder.writes.is_empty());
    for (method, id) in &recorder.writes {
        assert!(
            id == "task-menu" || TASK_MENU_HTML.contains(&format!("id=\"{id}\"")),
            "{method} writes `{id}`, which the task menu markup does not define"
        );
    }
}

#[test]
fn task_menu_projection_never_writes_a_border() {
    let recorder = project_idle();
    assert!(
        recorder
            .writes
            .iter()
            .all(|(method, _)| !method.starts_with("border")),
        "task menu frame comes from the shared sheet; projection must not paint a border: {:?}",
        recorder.writes
    );
}

#[test]
fn start_app_menu_shows_the_row_matching_the_favorite_state() {
    let state = crate::ContextMenuState {
        kind: crate::ContextMenuKind::StartApp(flamewm_api::DesktopAppId::new("org.example.calc")),
        revision: 0,
        output: None,
        interaction: None,
        anchor: None,
    };
    let mut recorder = Recorder::default();
    project_context_menu(
        &mut recorder,
        &ShellSnapshot::default(),
        &SettingsSnapshot::new(0),
        Some(&state),
        &["org.example.calc".to_owned()],
        0,
    )
    .expect("projection");
    // Favorited app: the remove row is projected, the add row is hidden.
    assert!(recorder
        .writes
        .contains(&("visible", "context-start-remove".to_owned())));
    assert!(recorder
        .writes
        .contains(&("visible", "context-start-add".to_owned())));
    assert!(TASK_MENU_HTML.contains("id=\"context-start-add\""));
    assert!(TASK_MENU_HTML.contains("id=\"context-start-remove\""));
}
