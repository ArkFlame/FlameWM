//! Markup contract for the Settings window: every node id the controller writes must exist in
//! `ui/settings.html`. The strict document below rejects unknown ids the way the real
//! `UiDocument` does, so a missing node fails here instead of in a running session.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::OnceLock;

use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::shortcuts::ShortcutSnapshot;
use flamewm_api::{ModeId, OutputId, Rect, Size};
use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
use flamewm_settings_core::{ControlTransport, DISPLAY_MODE_SLOTS};
use flamewm_ui_core::style::UiLayer;
use flamewm_ui_x11::{
    Overflow, RuntimeImage, UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess,
};

use crate::{ACCENT_NODE_IDS, OUTPUT_PAGE_SLOTS, SETTING_TWINS, SettingsApplication};

const MARKUP: &str = include_str!("../../../ui/settings.html");

/// Every value of `attribute="..."` declared in the Settings markup, in document order.
fn markup_attr(attribute: &str) -> Vec<&'static str> {
    MARKUP
        .split(format!("{attribute}=\"").as_str())
        .skip(1)
        .filter_map(|fragment| fragment.split('"').next())
        .collect()
}

/// Every `id="..."` declared in the Settings markup, parsed once.
fn markup_ids() -> &'static BTreeSet<String> {
    static IDS: OnceLock<BTreeSet<String>> = OnceLock::new();
    IDS.get_or_init(|| markup_attr("id").into_iter().map(str::to_owned).collect())
}

/// Records every write and rejects ids that the markup does not declare.
#[derive(Default)]
struct StrictDocument {
    ids: BTreeSet<String>,
    texts: BTreeMap<String, String>,
    visible: BTreeMap<String, bool>,
    backgrounds: BTreeMap<String, UiColor>,
    rejected: Vec<String>,
}

impl StrictDocument {
    fn new() -> Self {
        Self {
            ids: markup_ids().clone(),
            ..Self::default()
        }
    }

    fn known(&mut self, id: &str) -> Result<(), String> {
        if self.ids.contains(id) {
            Ok(())
        } else {
            let error = format!("no item {id}");
            self.rejected.push(error.clone());
            Err(error)
        }
    }
}

impl UiDocumentAccess for StrictDocument {
    fn text(&mut self, id: &str, text: String) -> Result<(), String> {
        self.known(id)?;
        self.texts.insert(id.to_owned(), text);
        Ok(())
    }
    fn image_rgb8(&mut self, id: &str, _image: RuntimeImage) -> Result<(), String> {
        self.known(id)
    }
    fn image_rgba8(&mut self, id: &str, _image: RuntimeImage) -> Result<(), String> {
        self.known(id)
    }
    fn image_reset(&mut self, id: &str) -> Result<(), String> {
        self.known(id)
    }
    fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        self.known(id)?;
        self.visible.insert(id.to_owned(), visible);
        Ok(())
    }
    fn toggle(&mut self, track_id: &str, knob_id: &str, _checked: bool) -> Result<(), String> {
        self.known(track_id)?;
        self.known(knob_id)
    }
    fn position(&mut self, id: &str, _left: f32, _top: f32) -> Result<(), String> {
        self.known(id)
    }
    fn size(&mut self, id: &str, _width: f32, _height: f32) -> Result<(), String> {
        self.known(id)
    }
    fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.known(id)?;
        self.backgrounds.insert(id.to_owned(), color);
        Ok(())
    }
    fn border(&mut self, id: &str, _color: UiColor) -> Result<(), String> {
        self.known(id)
    }
    fn corner_radius(&mut self, id: &str, _radius_px: f32) -> Result<(), String> {
        self.known(id)
    }
    fn node_backdrop(&mut self, id: &str, _blur_percent: Option<u8>) -> Result<(), String> {
        self.known(id)
    }
    fn foreground(&mut self, id: &str, _color: UiColor) -> Result<(), String> {
        self.known(id)
    }
    fn background_clear(&mut self, id: &str) -> Result<(), String> {
        self.known(id)?;
        self.backgrounds.remove(id);
        Ok(())
    }
    fn border_clear(&mut self, id: &str) -> Result<(), String> {
        self.known(id)
    }
    fn foreground_clear(&mut self, id: &str) -> Result<(), String> {
        self.known(id)
    }
    fn font_size(&mut self, id: &str, _size_px: f32) -> Result<(), String> {
        self.known(id)
    }
    fn font_weight(&mut self, id: &str, _weight: u16) -> Result<(), String> {
        self.known(id)
    }
    fn overflow(&mut self, id: &str, _x: Overflow, _y: Overflow) -> Result<(), String> {
        self.known(id)
    }
    fn layer(&mut self, id: &str, _layer: UiLayer) -> Result<(), String> {
        self.known(id)
    }
}

#[derive(Default)]
struct ScriptedTransport {
    responses: VecDeque<ControlResponse>,
    requests: Rc<RefCell<Vec<ControlRequest>>>,
}

impl ControlTransport for ScriptedTransport {
    fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
        self.requests.borrow_mut().push(request);
        // Unscripted calls answer with a wrong-shape reply, so the controller reports an error.
        Ok(self.responses.pop_front().unwrap_or(ControlResponse::Pong))
    }
}

fn output(id: &str, primary: bool, modes: usize) -> OutputSnapshot {
    let modes_list: Vec<DisplayMode> = (0..modes)
        .map(|index| DisplayMode {
            id: ModeId(index as u64 + 1),
            resolution: Size::new(1920 - index as i32 * 16, 1080 - index as i32 * 9),
            refresh_millihz: 60_000,
            preferred: index == 0,
        })
        .collect();
    OutputSnapshot {
        id: OutputId::new(id),
        connector: id.to_owned(),
        edid_identity: format!("fixture-{id}"),
        connected: true,
        primary,
        geometry: Rect::new(0, 0, 1920, 1080),
        current_mode: ModeId(1),
        modes: modes_list,
        shell_scale_percent: 100,
    }
}

/// Several fixtures: one output, a full output page, and a paged mode list.
fn display_fixtures() -> Vec<DisplaySnapshot> {
    let snapshot = |generation, outputs| DisplaySnapshot {
        generation,
        outputs,
        pending: None,
    };
    vec![
        snapshot(1, vec![output("HDMI-1", true, 1)]),
        snapshot(
            2,
            (0..OUTPUT_PAGE_SLOTS)
                .map(|slot| output(&format!("OUT-{slot}"), slot == 0, 2))
                .collect(),
        ),
        snapshot(3, vec![output("DP-1", true, DISPLAY_MODE_SLOTS + 3)]),
    ]
}

fn settings_fixture(accent: &str, appearance: &str) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot
        .values
        .insert("accent".to_owned(), SettingValue::Text(accent.to_owned()));
    snapshot.values.insert(
        "appearance".to_owned(),
        SettingValue::Text(appearance.to_owned()),
    );
    for (key, _, _, default) in SETTING_TWINS {
        snapshot
            .values
            .insert((*key).to_owned(), SettingValue::Boolean(*default));
    }
    snapshot
}

fn application(
    settings: SettingsSnapshot,
    displays: DisplaySnapshot,
) -> SettingsApplication<ScriptedTransport> {
    let transport = ScriptedTransport {
        responses: VecDeque::from([
            ControlResponse::Settings(settings),
            ControlResponse::Displays(displays),
            ControlResponse::Shortcuts(ShortcutSnapshot {
                revision: 1,
                bindings: Default::default(),
            }),
            ControlResponse::Panels(flamewm_api::panels::PanelsSnapshot {
                revision: 1,
                panels: Vec::new(),
                tasks: Vec::new(),
                pinned_apps: Vec::new(),
            }),
        ]),
        requests: Rc::new(RefCell::new(Vec::new())),
    };
    let mut application = SettingsApplication::new(transport);
    application
        .client_mut()
        .refresh_all()
        .expect("fixture refresh");
    application
}

fn release(action: &str) -> UiActionEvent {
    UiActionEvent {
        action: action.to_owned(),
        phase: UiActionPhase::Release,
        inside: true,
        button: 1,
        x: 0.0,
        y: 0.0,
        text: None,
    }
}

#[test]
fn markup_ids_are_unique() {
    let mut seen = BTreeSet::new();
    for id in markup_attr("id") {
        assert!(seen.insert(id), "duplicate id in settings.html: {id}");
    }
}

#[test]
fn every_static_node_the_controller_writes_is_declared_in_markup() {
    let ids = markup_ids();
    for id in ACCENT_NODE_IDS {
        assert!(ids.contains(*id), "ACCENT_NODE_IDS names missing node {id}");
    }
    for (key, on, off, _) in SETTING_TWINS {
        assert!(ids.contains(*on), "{key} ON twin missing: {on}");
        assert!(ids.contains(*off), "{key} OFF twin missing: {off}");
    }
    for node in ["nav-selection", "settings-error"] {
        assert!(ids.contains(node), "missing node {node}");
    }
    for slot in 0..OUTPUT_PAGE_SLOTS {
        for prefix in [
            "display-output",
            "display-output-name",
            "display-output-size",
            "display-output-selected",
            "display-output-unselected",
            "taskbar-output",
        ] {
            let id = format!("{prefix}-{slot}");
            assert!(ids.contains(&id), "missing slot node {id}");
        }
    }
    for slot in 0..DISPLAY_MODE_SLOTS {
        let id = format!("display-mode-slot-{slot}");
        assert!(ids.contains(&id), "missing mode slot node {id}");
    }
}

#[test]
fn every_markup_action_parses_to_a_controller_action() {
    let actions = markup_attr("data-action");
    assert!(!actions.is_empty(), "settings markup declares actions");
    for action in actions {
        assert!(
            crate::parse_action(action).is_ok(),
            "orphan action in settings markup: {action}"
        );
    }
}

#[test]
fn sync_writes_content_without_unknown_ids_on_every_fixture() {
    let accents = ["#EF4048", "#3DAEE9"];
    for displays in display_fixtures() {
        for accent in accents {
            let app = application(settings_fixture(accent, "dark"), displays.clone());
            let mut document = StrictDocument::new();
            app.sync(&mut document)
                .unwrap_or_else(|error| panic!("sync failed on strict markup: {error}"));
            assert!(
                document.rejected.is_empty(),
                "unknown ids: {:?}",
                document.rejected
            );
            assert_eq!(
                document
                    .backgrounds
                    .get("nav-selection-accent")
                    .copied()
                    .map(|c| (c.r, c.g, c.b)),
                Some(hex_rgb(accent)),
                "sidebar accent line follows the live accent"
            );
            assert!(document.visible.contains_key("page-desktop"));
            assert!(document.texts.contains_key("display-output-name-0"));
        }
    }
}

#[test]
fn one_failing_optional_node_does_not_block_other_writes() {
    let mut document = StrictDocument::new();
    document.ids.remove("nav-selection-accent");
    let app = application(
        settings_fixture("#EF4048", "dark"),
        display_fixtures().remove(0),
    );
    let error = app
        .sync(&mut document)
        .expect_err("a missing accent node is reported");
    assert!(error.contains("no item nav-selection-accent"), "{error}");
    assert!(
        document.visible.contains_key("page-desktop"),
        "later steps still ran"
    );
    assert!(
        document.texts.contains_key("display-output-name-0"),
        "display page still ran"
    );
}

#[test]
fn every_navigation_page_syncs_on_the_strict_document() {
    let mut app = application(
        settings_fixture("#EF4048", "dark"),
        display_fixtures().remove(1),
    );
    for action in markup_attr("data-action")
        .into_iter()
        .filter(|a| a.starts_with("settings.page."))
    {
        let mut document = StrictDocument::new();
        app.handle_action(&release(action), &mut document)
            .unwrap_or_else(|error| panic!("{action} failed: {error}"));
        assert!(
            document.rejected.is_empty(),
            "{action}: unknown ids {:?}",
            document.rejected
        );
    }
}

#[test]
fn compiled_runtime_document_accepts_every_sync_write() {
    // The shipped document: settings.html compiled at build time, loaded by `view::document`.
    let mut document = crate::view::document().expect("compiled settings document");
    for displays in display_fixtures() {
        let app = application(settings_fixture("#3DAEE9", "light"), displays);
        app.sync(&mut document)
            .unwrap_or_else(|error| panic!("runtime document rejected a sync write: {error}"));
    }
}

#[test]
fn every_visible_mode_slot_resolves_on_every_page() {
    for displays in display_fixtures() {
        let modes = displays.outputs[0].modes.len();
        let pages = modes.div_ceil(DISPLAY_MODE_SLOTS);
        let mut app = application(settings_fixture("#EF4048", "dark"), displays);
        for page in 0..pages {
            let mut document = StrictDocument::new();
            app.sync(&mut document).expect("page sync");
            let visible: Vec<usize> = (0..DISPLAY_MODE_SLOTS)
                .filter(|slot| {
                    document.visible.get(&format!("display-mode-slot-{slot}")) == Some(&true)
                })
                .collect();
            assert_eq!(
                visible.len(),
                (modes - page * DISPLAY_MODE_SLOTS).min(DISPLAY_MODE_SLOTS)
            );
            for slot in visible {
                let mut sink = StrictDocument::new();
                if let Err(error) = app.handle_action(
                    &release(&format!("settings.display.slot.{slot}")),
                    &mut sink,
                ) {
                    assert!(
                        !error.contains("no display mode") && !error.contains("stale"),
                        "page {page} visible slot {slot} rejected: {error}"
                    );
                }
            }
            let mut next = StrictDocument::new();
            app.handle_action(&release("settings.display.modes.next"), &mut next)
                .expect("mode page step");
        }
    }
}

fn hex_rgb(value: &str) -> (u8, u8, u8) {
    let digits = value.trim_start_matches('#');
    let channel =
        |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).expect("hex");
    (channel(0..2), channel(2..4), channel(4..6))
}

/// Fails for any id the Settings markup does not declare. Test fakes call this on every write.
pub(crate) fn require_markup_id(id: &str) -> Result<(), String> {
    if markup_ids().contains(id) {
        Ok(())
    } else {
        Err(format!("no item {id}"))
    }
}
