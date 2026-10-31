//! Split from `lib.rs` (`actions`); see crate docs.

use super::*;

/// Presentation sub-pages that the Appearance nav rows open. They share the core
/// `SettingsPage::Appearance` and are local to this window, never WM state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppearanceTab {
    Theme,
    StartMenu,
    Surfaces,
    Animations,
    LockScreen,
    Screensaver,
}

impl AppearanceTab {
    pub(crate) const ALL: [Self; 6] = [
        Self::Theme,
        Self::StartMenu,
        Self::Surfaces,
        Self::Animations,
        Self::LockScreen,
        Self::Screensaver,
    ];

    /// The markup suffix: nav action `settings.page.<node>` and section `page-<node>`.
    pub(crate) fn node(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::StartMenu => "start-menu",
            Self::Surfaces => "surfaces",
            Self::Animations => "animations",
            Self::LockScreen => "lock-screen",
            Self::Screensaver => "screensaver",
        }
    }

    fn parse(node: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.node() == node)
    }
}

/// Every control action the Settings markup may emit. Parsing is pure, so the
/// markup coverage test can check each `data-action` without side effects.
pub(crate) enum SettingsAction {
    FileManagerCycle,
    Page(SettingsPage),
    AccentSwatch(&'static str),
    /// Shows one Appearance sub-page from the nav.
    AppearanceTab(AppearanceTab),
    /// Opens the Start icon chooser. Import is not wired yet (see `execute`).
    StartIconChoose,
    /// Clears the Start icon and removes its managed copy.
    StartIconClear,
    /// Opens the icon or cursor theme picker on its first page.
    ThemePickerOpen(theme_picker::PickerKind),
    ThemePickerClose,
    ThemePickerPage(theme_picker::PickerKind, PageStep),
    /// Row `slot` on the open picker's current page.
    ThemePickerSelect(theme_picker::PickerKind, usize),
    /// Opens the portal chooser for one archive, or one folder when `folder` is set.
    ThemeInstallChoose {
        folder: bool,
    },
    WallpaperChoose,
    /// Shows the bundled wallpaper; the saved custom path is kept.
    WallpaperDefault,
    /// Shows the solid wallpaper colour; the saved custom path is kept.
    WallpaperClear,
    /// Opens the shared file chooser for the lock-screen background image.
    LockBackgroundChoose,
    /// Shows the solid lock colour; the saved custom path is kept.
    LockBackgroundClear,
    /// Shows the bundled lock background; the saved custom path is kept.
    LockBackgroundDefault,
    /// Opens the shared file chooser for the lock-screen profile picture.
    AvatarChoose,
    AvatarClear,
    /// Starts the lock screen in preview mode through the desktop control bus.
    LockPreview,
    /// Starts the screensaver in preview mode through the desktop control bus.
    ScreensaverPreview,
    /// Writes one fixed integer value to a setting, from an explicit option button.
    IntegerSelect {
        key: &'static str,
        value: i64,
    },
    Toggle(&'static str),
    SelectionOpacity(i8),
    SnapOpacity(i8),
    DesktopEntrySize(i8),
    FontSize(i8),
    TaskbarEdgeCycle,
    TaskbarEdge(PanelEdge),
    TaskbarSizeCycle,
    FontFamilyCycle,
    Hotkey(&'static str, HotkeyOperation),
    /// Output row `slot` on the current output page.
    DisplaySelect(usize),
    DisplayArrange(display_layout::Side),
    /// Mode row `slot` on the current mode page.
    DisplaySlot(usize),
    OutputPage(PageStep),
    ModePage(PageStep),
    DisplayBegin(ModeId),
    DisplayScale(u16),
    DisplayKeep,
    DisplayRevert,
    OpenUri(&'static str),
    /// Opens the shared colour picker for one `#RRGGBB` setting.
    ColorPickerOpen(&'static str),
    ColorPickerClose,
    /// Focuses the picker's hex field so typed keys edit its buffer.
    ColorPickerFocus,
    /// Applies one palette cell (`#RRGGBB`) to the picker's setting at once.
    ColorPickerPick(String),
    /// Validates the hex buffer and applies it; an invalid buffer writes nothing.
    ColorPickerApply,
    /// Opens the API key draft (seeded from the saved key) so typed keys edit it.
    ApiKeyFocus,
    /// Clears `assistant.apiKey` so the assistant uses the built-in default key.
    ApiKeyReset,
    /// Spawns `flamewm-assistant`, which toggles its own window.
    AssistantOpen,
    /// Sets a `#RRGGBB` setting to one fixed preset value in one transaction.
    ColorSwatch {
        key: &'static str,
        value: &'static str,
    },
    /// Steps a percent setting by 5 within 0..=100.
    PercentStep(&'static str, i8),
    /// Writes every surface glass option of one preset in one transaction.
    GlassPreset(glass::GlassPreset),
    /// Resets the `glass` settings section only; surface and window keys are kept.
    ResetGlass,
    /// Focuses the sidebar search field so typed text filters the page list.
    SearchFocus,
    /// Adds `delta` to an integer setting and clamps the result to `minimum..=maximum`.
    IntegerStep {
        key: &'static str,
        delta: i64,
        minimum: i64,
        maximum: i64,
    },
    /// Drag-only slider; the value arrives as event text, see `handle_animation_speed_drag`.
    AnimationSpeedSlider,
    /// Drag-only selected output scale slider; only Release changes runtime scale.
    DisplayScaleSlider,
    /// Drag-only glass slider in `GLASS_SLIDERS`; the value arrives as event text, see
    /// `handle_glass_slider_drag`.
    GlassSlider,
    /// Advances a text setting through `values`, wrapping.
    TextCycle {
        key: &'static str,
        values: &'static [&'static str],
    },
    /// Writes one fixed text value to a setting, from an explicit option button.
    TextSelect {
        key: &'static str,
        value: &'static str,
    },
    /// A native `keyboard.*` id from the X11 key source. It is not markup, so
    /// it never edits a row; hotkey capture is driven by the key-event path.
    NativeKeyEvent,
}
pub(crate) fn accent_color(value: &str) -> Result<UiColor, String> {
    let digits = value
        .strip_prefix('#')
        .filter(|digits| digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| format!("accent is not a #RRGGBB value: '{value}'"))?;
    let channel = |range: std::ops::Range<usize>, name: &str| {
        digits
            .get(range)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            .ok_or_else(|| format!("invalid accent {name}"))
    };
    Ok(UiColor {
        r: channel(0..2, "red")?,
        g: channel(2..4, "green")?,
        b: channel(4..6, "blue")?,
        a: 56,
    })
}

pub(crate) fn page(value: &str) -> Option<SettingsPage> {
    match value {
        "appearance" => Some(SettingsPage::Appearance),
        "desktop" => Some(SettingsPage::Desktop),
        "taskbar" => Some(SettingsPage::Taskbar),
        "displays" => Some(SettingsPage::Displays),
        "fonts" => Some(SettingsPage::Fonts),
        "hotkeys" => Some(SettingsPage::Hotkeys),
        "assistant" => Some(SettingsPage::Assistant),
        "about" => Some(SettingsPage::About),
        _ => None,
    }
}

pub(crate) fn control_error(error: ControlError) -> String {
    format!("{}: {}", error.name, error.message)
}

/// Asks the desktop to start a preview over the control bus.
/// The desktop owns the preview process; Settings only sends the typed request.
pub(crate) fn start_session_preview(action: SessionAction) -> Result<(), String> {
    let mut control = control::SettingsControl::connect_session()
        .map_err(|error| format!("could not reach the desktop: {error}"))?;
    control
        .call(ControlRequest::SessionAction(action))
        .map(|_| ())
        .map_err(control_error)
}

pub(crate) fn about_uri(action: &str) -> Option<&'static str> {
    match action {
        "about.donate" => Some(DONATE_URI),
        "about.source" => Some(SOURCE_URI),
        "about.website" => Some(WEBSITE_URI),
        _ => None,
    }
}

/// Parses the rest after `settings.theme.`: `close`, `install`, or `<icon|cursor>.<op>`
/// where `<op>` is `open`, `prev`, `next` or `select.<slot>`.
pub(crate) fn parse_theme_action(rest: &str) -> Result<SettingsAction, String> {
    match rest {
        "close" => return Ok(SettingsAction::ThemePickerClose),
        "install" => return Ok(SettingsAction::ThemeInstallChoose { folder: false }),
        "install-folder" => return Ok(SettingsAction::ThemeInstallChoose { folder: true }),
        _ => {}
    }
    let (target, operation) = rest
        .split_once('.')
        .ok_or_else(|| format!("unknown theme action '{rest}'"))?;
    let kind = theme_picker::PickerKind::parse(target)
        .ok_or_else(|| format!("unknown theme target '{target}'"))?;
    match operation {
        "open" => Ok(SettingsAction::ThemePickerOpen(kind)),
        "prev" => Ok(SettingsAction::ThemePickerPage(kind, PageStep::Prev)),
        "next" => Ok(SettingsAction::ThemePickerPage(kind, PageStep::Next)),
        _ => {
            let slot = operation
                .strip_prefix("select.")
                .and_then(|slot| slot.parse::<usize>().ok())
                .filter(|slot| *slot < theme_picker::PICKER_SLOTS)
                .ok_or_else(|| format!("unknown theme operation '{operation}'"))?;
            Ok(SettingsAction::ThemePickerSelect(kind, slot))
        }
    }
}

/// Parses a numeric action suffix that must be one of `choices`; `label` names the setting in errors.
pub(crate) fn integer_choice_action(
    key: &'static str,
    choices: &[i64],
    raw: &str,
    label: &str,
) -> Result<SettingsAction, String> {
    raw.parse::<i64>()
        .ok()
        .filter(|value| choices.contains(value))
        .map(|value| SettingsAction::IntegerSelect { key, value })
        .ok_or_else(|| format!("unknown {label} '{raw}'"))
}

/// Keys that `settings.toggle.<key>` may flip, in no particular order.
pub(crate) const TOGGLE_KEYS: [&str; 36] = [
    "reduceTransparency",
    "showContentWhileDragging",
    "stickyNotesEnabled",
    "organizeDownloads",
    "fontBold",
    "animationsEnabled",
    "startAnimationEnabled",
    "windowOpenAnimationEnabled",
    "minimizeAnimationEnabled",
    "maximizeAnimationEnabled",
    "lockShowDate",
    "lockShowUsername",
    "lockOnSuspend",
    "taskbarShowSeconds",
    "taskbarShowDate",
    "taskbarShowWeather",
    "taskbarShowVolume",
    "taskbarShowNetwork",
    "taskbarShowBattery",
    "taskbarBatteryOnlyWhenRelevant",
    "taskbarShowClock",
    "taskbarShowWorkspaces",
    "weatherEnabled",
    "autoLocationEnabled",
    "autoTimeZoneEnabled",
    "snapPreviewEnabled",
    "focusFollowsMouse",
    "shareWallpaper",
    "accentCustom",
    "titlebars",
    "taskbarActivities",
    "taskbarSeparate",
    "assistant.enabled",
    "assistant.taskbarIcon",
    "assistant.speechToText",
    "assistant.textToSpeech",
];

/// Speech toggles whose turn-on starts the background voice setup (models download once).
pub(crate) const SPEECH_TOGGLES: [&str; 2] = ["assistant.speechToText", "assistant.textToSpeech"];

/// Starts `flamewm-assistant --voice-setup` without waiting for it. A waiter thread reaps the child.
/// The setup is idempotent: a finished or already running setup makes the new process exit at once.
pub(crate) fn start_voice_setup() -> Result<(), String> {
    let mut child = std::process::Command::new("flamewm-assistant")
        .arg("--voice-setup")
        .spawn()
        .map_err(|error| format!("Could not start the voice setup: {error}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Parses one `data-action` value. Unknown IDs are errors, never silent no-ops.
pub(crate) fn parse_action(action: &str) -> Result<SettingsAction, String> {
    if action == "settings.default-file-manager.cycle" {
        return Ok(SettingsAction::FileManagerCycle);
    }
    if action.starts_with("keyboard.") {
        return Ok(SettingsAction::NativeKeyEvent);
    }
    if let Some(tab) = action
        .strip_prefix("settings.page.")
        .and_then(AppearanceTab::parse)
    {
        return Ok(SettingsAction::AppearanceTab(tab));
    }
    if let Some(page) = action.strip_prefix("settings.page.").and_then(page) {
        return Ok(SettingsAction::Page(page));
    }
    if let Some(name) = action.strip_prefix("settings.accent.") {
        return accent_for_swatch(name)
            .map(SettingsAction::AccentSwatch)
            .ok_or_else(|| format!("unknown accent '{name}'"));
    }
    if let Some(edge) = action.strip_prefix("settings.taskbar.position.") {
        return match edge {
            "bottom" => Ok(SettingsAction::TaskbarEdge(PanelEdge::Bottom)),
            "top" => Ok(SettingsAction::TaskbarEdge(PanelEdge::Top)),
            "left" => Ok(SettingsAction::TaskbarEdge(PanelEdge::Left)),
            "right" => Ok(SettingsAction::TaskbarEdge(PanelEdge::Right)),
            _ => Err(format!("unknown panel edge '{edge}'")),
        };
    }
    if let Some(key) = action.strip_prefix("settings.toggle.") {
        return TOGGLE_KEYS
            .into_iter()
            .find(|known| *known == key)
            .map(SettingsAction::Toggle)
            .ok_or_else(|| format!("unknown toggle '{key}'"));
    }
    if let Some(rest) = action.strip_prefix("settings.color.") {
        return parse_color_preset(rest);
    }
    if let Some(rest) = action.strip_prefix("settings.picker.") {
        return parse_color_picker(rest);
    }
    if let Some(direction) = action
        .strip_prefix("settings.surface-border-opacity.")
        .and_then(step)
    {
        return Ok(SettingsAction::PercentStep(
            "surfaceBorderOpacity",
            direction,
        ));
    }
    if let Some(direction) = action.strip_prefix("settings.surface-blur.").and_then(step) {
        return Ok(SettingsAction::PercentStep("surfaceBlurPercent", direction));
    }
    for (slug, key) in GLASS_STEP_KEYS {
        if let Some(direction) = action
            .strip_prefix(slug)
            .and_then(step)
        {
            return Ok(SettingsAction::PercentStep(key, direction));
        }
    }
    match action {
        "settings.glass.dark" => return Ok(SettingsAction::GlassPreset(glass::DARK)),
        "settings.glass.light" => return Ok(SettingsAction::GlassPreset(glass::LIGHT)),
        "settings.glass.reset" => return Ok(SettingsAction::ResetGlass),
        "settings.search.focus" => return Ok(SettingsAction::SearchFocus),
        _ => {}
    }
    if let Some(direction) = action
        .strip_prefix("settings.surface-opacity.")
        .and_then(step)
    {
        return Ok(SettingsAction::PercentStep("surfaceOpacity", direction));
    }
    if let Some((slug, direction)) = action
        .strip_prefix("settings.")
        .and_then(|rest| rest.rsplit_once('.'))
        .filter(|(slug, _)| INTEGER_STEPPERS.iter().any(|row| row.slug == *slug))
    {
        let row = INTEGER_STEPPERS
            .iter()
            .find(|row| row.slug == slug)
            .ok_or_else(|| format!("unknown stepper '{slug}'"))?;
        let direction =
            step(direction).ok_or_else(|| format!("unknown step '{direction}' for {slug}"))?;
        return Ok(SettingsAction::IntegerStep {
            key: row.key,
            delta: i64::from(direction) * row.step,
            minimum: row.minimum,
            maximum: row.maximum,
        });
    }
    if let Some(direction) = action
        .strip_prefix("settings.accent-opacity.")
        .and_then(step)
    {
        return Ok(SettingsAction::PercentStep("accentOpacity", direction));
    }
    if action == ANIMATION_SPEED_SLIDER {
        return Ok(SettingsAction::AnimationSpeedSlider);
    }
    if glass_slider(action).is_some() {
        return Ok(SettingsAction::GlassSlider);
    }
    if action == DISPLAY_SCALE_SLIDER {
        return Ok(SettingsAction::DisplayScaleSlider);
    }
    if let Some(direction) = action
        .strip_prefix("settings.selection-opacity.")
        .and_then(step)
    {
        return Ok(SettingsAction::SelectionOpacity(direction));
    }
    if let Some(direction) = action.strip_prefix("settings.snap-opacity.").and_then(step) {
        return Ok(SettingsAction::SnapOpacity(direction));
    }
    if let Some(direction) = action
        .strip_prefix("settings.desktop-entry-size.")
        .and_then(step)
    {
        return Ok(SettingsAction::DesktopEntrySize(direction));
    }
    if let Some(direction) = action.strip_prefix("settings.font-size.").and_then(step) {
        return Ok(SettingsAction::FontSize(direction));
    }
    if action == "settings.display.arrange.left" {
        return Ok(SettingsAction::DisplayArrange(display_layout::Side::Left));
    }
    if action == "settings.display.arrange.right" {
        return Ok(SettingsAction::DisplayArrange(display_layout::Side::Right));
    }
    if let Some(slot) = action.strip_prefix("settings.display.select.") {
        return slot
            .parse::<usize>()
            .map(SettingsAction::DisplaySelect)
            .map_err(|_| format!("invalid display slot '{slot}'"));
    }
    if let Some(slot) = action.strip_prefix("settings.display.slot.") {
        return slot
            .parse::<usize>()
            .map(SettingsAction::DisplaySlot)
            .map_err(|_| format!("invalid display mode slot '{slot}'"));
    }
    if let Some(raw_mode) = action
        .strip_prefix("settings.display.begin.")
        .or_else(|| action.strip_prefix("settings.display.mode."))
    {
        return raw_mode
            .parse::<u64>()
            .map(|mode| SettingsAction::DisplayBegin(ModeId(mode)))
            .map_err(|_| format!("invalid display mode '{raw_mode}'"));
    }
    if let Some(percent) = action.strip_prefix("settings.display.scale.") {
        return percent
            .parse::<u16>()
            .map(SettingsAction::DisplayScale)
            .map_err(|_| format!("invalid display scale '{percent}'"));
    }
    if let Some(rest) = action.strip_prefix("settings.hotkey.") {
        let (slot, operation) = match rest.split_once('.') {
            Some((slot, "clear")) => (slot, HotkeyOperation::Clear),
            Some((slot, "reset")) => (slot, HotkeyOperation::Reset),
            Some(_) => return Err(format!("unknown hotkey operation '{rest}'")),
            None => (rest, HotkeyOperation::Capture),
        };
        let row = hotkey_row(slot).ok_or_else(|| format!("unknown hotkey row '{slot}'"))?;
        return Ok(SettingsAction::Hotkey(row.action, operation));
    }
    if let Some(uri) = about_uri(action) {
        return Ok(SettingsAction::OpenUri(uri));
    }
    if let Some(rest) = action.strip_prefix("settings.theme.") {
        return parse_theme_action(rest);
    }
    if let Some((key, value)) = action
        .strip_prefix("settings.")
        .and_then(|rest| rest.split_once('.'))
    {
        if let Some((row, values)) = ANIMATION_STYLE_ROWS.iter().find(|(row, _)| *row == key) {
            return values
                .iter()
                .find(|allowed| **allowed == value)
                .map(|allowed| SettingsAction::TextSelect {
                    key: row,
                    value: allowed,
                })
                .ok_or_else(|| format!("unknown animation style '{value}' for {key}"));
        }
    }
    if let Some(size) = action.strip_prefix("settings.cursor.size.") {
        return integer_choice_action("cursorSize", &CURSOR_SIZE_CHOICES, size, "cursor size");
    }
    if let Some(minutes) = action.strip_prefix("settings.lock.timeout.") {
        return integer_choice_action(
            "lockTimeoutMinutes",
            &LOCK_TIMEOUT_CHOICES,
            minutes,
            "lock timeout",
        );
    }
    match action {
        "settings.lock.clock.24h" => Ok(SettingsAction::TextSelect {
            key: "lockClockFormat",
            value: "24h",
        }),
        "settings.lock.clock.12h" => Ok(SettingsAction::TextSelect {
            key: "lockClockFormat",
            value: "12h",
        }),
        "settings.taskbar.style.classic" => Ok(SettingsAction::TextSelect {
            key: "taskbarStyle",
            value: "classic",
        }),
        "settings.taskbar.style.floating" => Ok(SettingsAction::TextSelect {
            key: "taskbarStyle",
            value: "floating",
        }),
        "settings.taskbar.clock.24h" => Ok(SettingsAction::TextSelect {
            key: "taskbarClockFormat",
            value: "24h",
        }),
        "settings.taskbar.clock.12h" => Ok(SettingsAction::TextSelect {
            key: "taskbarClockFormat",
            value: "12h",
        }),
        "settings.weather.units.celsius" => Ok(SettingsAction::TextSelect {
            key: "weatherUnits",
            value: "celsius",
        }),
        "settings.weather.units.fahrenheit" => Ok(SettingsAction::TextSelect {
            key: "weatherUnits",
            value: "fahrenheit",
        }),
        "settings.assistant.key.focus" => Ok(SettingsAction::ApiKeyFocus),
        "settings.assistant.key.reset" => Ok(SettingsAction::ApiKeyReset),
        "settings.assistant.open" => Ok(SettingsAction::AssistantOpen),
        "settings.weather.location.clear" => Ok(SettingsAction::TextSelect {
            key: "weatherLocation",
            value: "",
        }),
        "settings.wallpaper.fit" => Ok(SettingsAction::TextCycle {
            key: "wallpaperFit",
            values: &WALLPAPER_FITS,
        }),
        "settings.screensaver.mode.ribbons" => Ok(SettingsAction::TextSelect {
            key: "screensaverMode",
            value: "ribbons",
        }),
        "settings.screensaver.mode.none" => Ok(SettingsAction::TextSelect {
            key: "screensaverMode",
            value: "none",
        }),
        "settings.lock.background.choose" => Ok(SettingsAction::LockBackgroundChoose),
        "settings.lock.background.clear" => Ok(SettingsAction::LockBackgroundClear),
        "settings.lock.background.default" => Ok(SettingsAction::LockBackgroundDefault),
        "settings.avatar.choose" => Ok(SettingsAction::AvatarChoose),
        "settings.avatar.clear" => Ok(SettingsAction::AvatarClear),
        "settings.lock.preview" => Ok(SettingsAction::LockPreview),
        "settings.screensaver.preview" => Ok(SettingsAction::ScreensaverPreview),
        "settings.wallpaper.choose" => Ok(SettingsAction::WallpaperChoose),
        "settings.wallpaper.default" => Ok(SettingsAction::WallpaperDefault),
        "settings.wallpaper.clear" => Ok(SettingsAction::WallpaperClear),
        "settings.taskbar.position" => Ok(SettingsAction::TaskbarEdgeCycle),
        "settings.taskbar.size" => Ok(SettingsAction::TaskbarSizeCycle),
        "settings.font.family" => Ok(SettingsAction::FontFamilyCycle),
        "settings.decoration.rounded" => Ok(SettingsAction::TextSelect {
            key: "decoration",
            value: "rounded",
        }),
        "settings.appearance.apps.dark" => Ok(SettingsAction::TextSelect {
            key: "appearance",
            value: "dark",
        }),
        "settings.appearance.apps.light" => Ok(SettingsAction::TextSelect {
            key: "appearance",
            value: "light",
        }),
        "settings.decoration.squared" => Ok(SettingsAction::TextSelect {
            key: "decoration",
            value: "squared",
        }),
        "settings.taskbar.material.blur-only" => Ok(SettingsAction::TextSelect {
            key: "taskbarMaterialMode",
            value: "blur-only",
        }),
        "settings.taskbar.material.inherit" => Ok(SettingsAction::TextSelect {
            key: "taskbarMaterialMode",
            value: "inherit",
        }),
        "settings.taskbar.material.custom-tint" => Ok(SettingsAction::TextSelect {
            key: "taskbarMaterialMode",
            value: "custom-tint",
        }),
        "settings.render-acceleration.cpu" => Ok(SettingsAction::TextSelect {
            key: "renderAcceleration",
            value: "cpu",
        }),
        "settings.render-acceleration.gpu" => Ok(SettingsAction::TextSelect {
            key: "renderAcceleration",
            value: "gpu",
        }),
        "settings.start-icon.choose" => Ok(SettingsAction::StartIconChoose),
        "settings.start-icon.clear" => Ok(SettingsAction::StartIconClear),
        "settings.titlebar.button-style.circle" => Ok(SettingsAction::TextSelect {
            key: "titlebarButtonStyle",
            value: "circle",
        }),
        "settings.titlebar.button-style.square" => Ok(SettingsAction::TextSelect {
            key: "titlebarButtonStyle",
            value: "square",
        }),
        "settings.display.keep" => Ok(SettingsAction::DisplayKeep),
        "settings.display.revert" => Ok(SettingsAction::DisplayRevert),
        // The Taskbar output pager shares the Displays output cursor.
        "settings.display.outputs.prev" => Ok(SettingsAction::OutputPage(PageStep::Prev)),
        "settings.display.outputs.next" => Ok(SettingsAction::OutputPage(PageStep::Next)),
        "settings.display.modes.prev" => Ok(SettingsAction::ModePage(PageStep::Prev)),
        "settings.display.modes.next" => Ok(SettingsAction::ModePage(PageStep::Next)),
        _ => Err(format!("unknown settings action '{action}'")),
    }
}
