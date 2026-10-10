//! Semantic icon roles mapped to existing product assets; no state is synthesized here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconSource {
    pub path: &'static str,
    pub treatment: IconTreatment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconTreatment {
    Original,
    SymbolicForeground,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconRole {
    Start,
    Home,
    Downloads,
    Projects,
    Browser,
    Trash,
    Settings,
    Terminal,
    Files,
    Code,
    Appearance,
    Desktop,
    Taskbar,
    Displays,
    Fonts,
    Hotkeys,
    About,
    Search,
    Folder,
    Note,
    Chevron,
    Minimize,
    Maximize,
    Restore,
    Close,
    Previous,
    Play,
    Pause,
    Next,
    Volume,
    VolumeMuted,
    Wifi,
    Lock,
    Logout,
    Reboot,
    Shutdown,
    /// Generic application glyph for app icon misses (never brand artwork).
    Application,
}

impl IconRole {
    #[must_use]
    pub const fn treatment(self) -> IconTreatment {
        match self {
            Self::Start | Self::Browser | Self::Terminal | Self::Files | Self::Code => {
                IconTreatment::Original
            }
            _ => IconTreatment::SymbolicForeground,
        }
    }

    #[must_use]
    pub const fn source(self) -> IconSource {
        let path = match self {
            Self::Start => "assets/web/flamewm-icon.svg",
            Self::Home => "assets/web/breeze/user-home.svg",
            Self::Downloads => "assets/web/breeze/folder-downloads.svg",
            Self::Projects => "assets/web/breeze/folder.svg",
            Self::Browser => "assets/web/breeze/internet-web-browser.svg",
            Self::Trash => "assets/web/breeze/user-trash.svg",
            Self::Settings => "assets/web/breeze/configure.svg",
            Self::Terminal => "assets/web/breeze/utilities-terminal.svg",
            Self::Files => "assets/web/breeze/system-file-manager.svg",
            Self::Code => "assets/web/breeze/applications-development.svg",
            Self::Appearance => "assets/web/breeze/settings-appearance.svg",
            Self::Desktop => "assets/web/breeze/settings-desktop.svg",
            Self::Taskbar => "assets/web/breeze/preferences-system.svg",
            Self::Displays => "assets/web/breeze/settings-displays.svg",
            Self::Fonts => "assets/web/breeze/settings-fonts.svg",
            Self::Hotkeys => "assets/web/breeze/settings-hotkeys.svg",
            Self::About => "assets/web/breeze/settings-about.svg",
            Self::Search => "assets/web/breeze/edit-find.svg",
            Self::Folder => "assets/web/breeze/folder-new.svg",
            Self::Note => "assets/web/sticky-note.svg",
            Self::Chevron => "assets/web/breeze/go-next.svg",
            Self::Minimize => "assets/web/breeze/window-minimize.svg",
            Self::Maximize => "assets/web/breeze/window-maximize.svg",
            Self::Restore => "assets/web/breeze/window-restore.svg",
            Self::Close => "assets/web/breeze/window-close.svg",
            Self::Previous => "assets/web/breeze/media-skip-backward.svg",
            Self::Play => "assets/web/breeze/media-playback-start.svg",
            Self::Pause => "assets/web/breeze/media-playback-pause.svg",
            Self::Next => "assets/web/breeze/media-skip-forward.svg",
            Self::Volume => "assets/web/breeze/audio-volume-high.svg",
            Self::VolumeMuted => "assets/web/breeze/audio-volume-muted.svg",
            Self::Wifi => "assets/web/breeze/network-wireless.svg",
            Self::Lock => "assets/web/breeze/system-lock-screen.svg",
            Self::Logout => "assets/web/breeze/system-log-out.svg",
            Self::Reboot => "assets/web/breeze/system-reboot.svg",
            Self::Shutdown => "assets/web/breeze/system-shutdown.svg",
            Self::Application => "assets/web/breeze/application-menu.svg",
        };
        IconSource {
            path,
            treatment: self.treatment(),
        }
    }
}
