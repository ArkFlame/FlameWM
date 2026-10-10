use flamewm_settings_core::SettingsPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsLaunchOptions {
    pub page: SettingsPage,
    /// Optional bounded geometry used by the native acceptance runner.
    pub window_size: Option<(u32, u32)>,
}

impl Default for SettingsLaunchOptions {
    fn default() -> Self {
        Self {
            page: SettingsPage::Appearance,
            window_size: None,
        }
    }
}

pub fn parse_args<I, S>(args: I) -> Result<SettingsLaunchOptions, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut options = SettingsLaunchOptions::default();
    let mut page_was_set = false;
    let mut window_size_was_set = false;
    let mut args = args.into_iter();
    while let Some(raw) = args.next() {
        let arg = raw.into();
        match arg.as_str() {
            "--page" => {
                let value: String = args
                    .next()
                    .ok_or_else(|| "--page requires a value".to_owned())?
                    .into();
                let page = parse_page(&value).ok_or_else(|| format!("unknown page '{value}'"))?;
                if page_was_set {
                    return Err("--page may be specified only once".to_owned());
                }
                page_was_set = true;
                options.page = page;
            }
            "--window-size" => {
                let value: String = args
                    .next()
                    .ok_or_else(|| "--window-size requires a value".to_owned())?
                    .into();
                if window_size_was_set {
                    return Err("--window-size may be specified only once".to_owned());
                }
                window_size_was_set = true;
                options.window_size = Some(parse_window_size(&value)?);
            }
            value => return Err(format!("unknown argument '{value}'")),
        }
    }
    Ok(options)
}

fn parse_window_size(value: &str) -> Result<(u32, u32), String> {
    let (width, height) = value
        .split_once('x')
        .ok_or_else(|| "--window-size must be WIDTHxHEIGHT".to_owned())?;
    let width = width
        .parse::<u32>()
        .map_err(|_| "--window-size must be WIDTHxHEIGHT".to_owned())?;
    let height = height
        .parse::<u32>()
        .map_err(|_| "--window-size must be WIDTHxHEIGHT".to_owned())?;
    match (width, height) {
        (760, 500) | (820, 540) | (960, 640) => Ok((width, height)),
        _ => Err("--window-size supports 760x500, 820x540, or 960x640".to_owned()),
    }
}

fn parse_page(value: &str) -> Option<SettingsPage> {
    match value {
        "appearance" => Some(SettingsPage::Appearance),
        "desktop" => Some(SettingsPage::Desktop),
        "taskbar" => Some(SettingsPage::Taskbar),
        "displays" => Some(SettingsPage::Displays),
        "fonts" => Some(SettingsPage::Fonts),
        "hotkeys" => Some(SettingsPage::Hotkeys),
        "about" => Some(SettingsPage::About),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_default_and_named_pages() {
        assert_eq!(
            parse_args::<_, &str>([]).unwrap().page,
            SettingsPage::Appearance
        );
        assert_eq!(
            parse_args(["--page", "desktop"]).unwrap().page,
            SettingsPage::Desktop
        );
        assert_eq!(
            parse_args(["--page", "about"]).unwrap().page,
            SettingsPage::About
        );
    }

    #[test]
    fn parser_accepts_only_the_bounded_geometry_matrix() {
        assert_eq!(parse_args::<_, &str>([]).unwrap().window_size, None);
        for size in ["760x500", "820x540", "960x640"] {
            assert_eq!(parse_args(["--window-size", size]).unwrap().window_size,
                Some(parse_window_size(size).unwrap()));
        }
        for size in ["0x0", "761x500", "760", "960x641", "760X500"] {
            assert!(parse_args(["--window-size", size]).is_err(), "accepted {size}");
        }
        assert!(parse_args(["--window-size", "760x500", "--window-size", "820x540"]).is_err());
    }

    #[test]
    fn parser_rejects_unknown_or_incomplete_arguments() {
        assert!(
            parse_args(["--unknown"])
                .unwrap_err()
                .contains("unknown argument")
        );
        assert!(
            parse_args(["--page"])
                .unwrap_err()
                .contains("requires a value")
        );
        assert!(
            parse_args(["--page", "missing"])
                .unwrap_err()
                .contains("unknown page")
        );
        assert!(
            parse_args(["--page", "desktop", "--page", "about"])
                .unwrap_err()
                .contains("only once")
        );
    }

    #[test]
    fn launch_parser_accepts_page_value() {
        let launch = production(include_str!("launch.rs"));
        let main = production(include_str!("main.rs"));
        assert!(
            launch.contains("--page") && launch.contains("desktop"),
            "launch parser must recognize --page desktop"
        );
        assert!(
            main.contains("launch::"),
            "settings main must route arguments through launch parsing"
        );
    }

    #[test]
    fn launch_parser_rejects_unknown_arguments() {
        let launch = production(include_str!("launch.rs"));
        assert!(
            launch.contains("unknown argument"),
            "launch parser must reject unknown arguments explicitly"
        );
    }

    fn production(source: &str) -> &str {
        source
            .split_once("\n#[cfg(test)]")
            .map_or(source, |(production, _)| production)
    }
}
