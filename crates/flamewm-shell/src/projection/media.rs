//! Media popover/tray projection: title fitting and the media panel record.

use flamewm_ui_x11::UiDocumentAccess;

use crate::ShellSnapshot;

/// Char budget for the media popup title. The title box is 276px wide with
/// 10px padding per side (256px usable) at 13px / weight 600; ~7.8px per glyph
/// gives ~32 glyphs. The render compiler has no text-overflow, so the ellipsis
/// is applied here. Counted with `chars()`, so multibyte text never splits.
const MEDIA_TITLE_MAX_CHARS: usize = 32;

/// Returns `title` unchanged when it fits `MEDIA_TITLE_MAX_CHARS`; otherwise
/// keeps the leading chars, trims trailing whitespace and appends `…` so the
/// total stays within the budget.
#[must_use]
fn fit_media_title(title: &str) -> String {
    if title.chars().count() <= MEDIA_TITLE_MAX_CHARS {
        return title.to_owned();
    }
    let kept: String = title.chars().take(MEDIA_TITLE_MAX_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

pub fn project_media(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let view = crate::taskbar::status::media::project(&snapshot.system);
    document.text("media-title", fit_media_title(&snapshot.system.media.title))?;
    document.visible(
        "media-play-icon",
        matches!(
            view.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPlay)
        ),
    )?;
    document.visible(
        "media-pause-icon",
        matches!(
            view.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPause)
        ),
    )?;
    let (can_previous, can_play_pause, can_next) =
        flamewm_shell_core::StatusPopovers::from_snapshot(&snapshot.system)
            .media
            .map_or((false, false, false), |media| {
                (media.can_previous, media.can_play_pause, media.can_next)
            });
    for (enabled, disabled, active) in [
        ("media-previous", "media-previous-disabled", can_previous),
        (
            "media-playpause",
            "media-playpause-disabled",
            can_play_pause,
        ),
        ("media-next", "media-next-disabled", can_next),
    ] {
        document.visible(enabled, active)?;
        document.visible(disabled, !active)?;
    }
    Ok(())
}

/// Element id of each transport stop, in Tab order; the focus ring is drawn on it.
pub const MEDIA_STOP_IDS: [&str; 3] = ["media-previous", "media-playpause", "media-next"];

/// Focus ring on the transport stop with Tab-order index `focus`; clears the ring from the rest.
pub fn project_media_focus(
    document: &mut impl UiDocumentAccess,
    focus: Option<usize>,
) -> Result<(), String> {
    for (index, id) in MEDIA_STOP_IDS.iter().enumerate() {
        if focus == Some(index) {
            document.border(id, crate::projection::task_accent())?;
        } else {
            document.border_clear(id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod media_projection_tests {
    use super::*;
    use crate::async_projection::tests::FakeDoc;

    // REGRESSION: document.border only sets colour; the transport stops need a 1px border
    // box in the CSS for the focus ring to paint, and every MEDIA_STOP_ID must exist in media.html.
    // MUTATION: removing `border:1px solid` from .media-control or renaming a stop id fails here.
    #[test]
    fn media_focus_ids_exist_in_html_with_a_ring_border_box() {
        let html = include_str!("../../../../ui/shell/media.html");
        let css = include_str!("../../../../ui/shell/flamewm.css");
        for id in MEDIA_STOP_IDS {
            assert!(
                html.contains(&format!("id=\"{id}\"")),
                "{id} missing from media.html"
            );
        }
        let rule = css
            .lines()
            .find(|line| line.starts_with(".media-control {"))
            .expect(".media-control rule");
        assert!(
            rule.contains("border:1px solid"),
            "ring needs a border box: {rule}"
        );
    }

    // REGRESSION: the popup has one title row and no artist line, so project_media
    // must not write `media-artist` (the compiled artifact no longer has that node).
    #[test]
    fn media_projects_title_only_without_artist_node() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.title = "Song Title".to_owned();
        snapshot.system.media.artist = "Some Artist".to_owned();
        let mut document = FakeDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        assert_eq!(
            document.texts.get("media-title").map(String::as_str),
            Some("Song Title")
        );
        assert!(
            !document.texts.contains_key("media-artist"),
            "artist line was removed from media.html"
        );
    }

    fn media_title_text(title: &str) -> String {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.title = title.to_owned();
        let mut document = FakeDoc::default();
        project_media(&mut document, &snapshot).expect("media projection must succeed");
        document
            .texts
            .get("media-title")
            .cloned()
            .expect("media-title must be written")
    }

    #[test]
    fn media_popup_projects_title_and_controls_only_no_artist_node() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.title = "Song Title".to_owned();
        snapshot.system.media.artist = "Some Artist".to_owned();
        snapshot.system.media.availability = flamewm_api::system::ServiceAvailability::Available;
        snapshot.system.media.playback = flamewm_api::system::PlaybackState::Playing;
        snapshot.system.media.can_previous = true;
        snapshot.system.media.can_next = true;
        let mut document = FakeDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        let mut text_ids: Vec<&str> = document.texts.keys().map(String::as_str).collect();
        text_ids.sort_unstable();
        assert_eq!(
            text_ids,
            ["media-title"],
            "popup text nodes must be title only"
        );
        assert!(!document.texts.contains_key("media-artist"));
        assert_eq!(document.visible.get("media-previous"), Some(&true));
        assert_eq!(document.visible.get("media-next"), Some(&true));
    }

    #[test]
    fn media_title_short_is_unchanged() {
        assert_eq!(media_title_text("Song Title"), "Song Title");
        let exact = "a".repeat(MEDIA_TITLE_MAX_CHARS);
        assert_eq!(media_title_text(&exact), exact);
    }

    #[test]
    fn media_title_long_ascii_is_truncated_with_ellipsis_within_budget() {
        let long = format!("{} tail words that keep going", "Abcdefghij ".repeat(5));
        let fitted = media_title_text(&long);
        assert!(fitted.ends_with('…'), "{fitted:?}");
        assert!(
            fitted.chars().count() <= MEDIA_TITLE_MAX_CHARS,
            "{fitted:?}"
        );
        assert!(fitted.starts_with("Abcdefghij Abcdefghij"), "{fitted:?}");
        assert!(!fitted.contains("  "), "{fitted:?}");
    }

    #[test]
    fn media_title_trims_whitespace_before_ellipsis() {
        // Budget keeps 31 chars: 30 'x' plus one space, which must be trimmed.
        let title = format!("{}     {}", "x".repeat(30), "y".repeat(10));
        let fitted = media_title_text(&title);
        assert_eq!(fitted, format!("{}…", "x".repeat(30)));
    }

    #[test]
    fn media_title_multibyte_and_emoji_do_not_panic_and_end_on_char_boundary() {
        let title = "🎵 Ünïcødé 日本語のタイトル 🎶 ".repeat(4);
        let fitted = media_title_text(&title);
        assert!(fitted.ends_with('…'), "{fitted:?}");
        assert!(
            fitted.chars().count() <= MEDIA_TITLE_MAX_CHARS,
            "{fitted:?}"
        );
        assert!(std::str::from_utf8(fitted.as_bytes()).is_ok());
    }

    // REGRESSION: a disabled control must show only its action-less twin; the
    // enabled button (which carries data-action) must be hidden, and vice versa.
    #[test]
    fn media_controls_swap_to_disabled_twins_by_capability() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.availability = flamewm_api::system::ServiceAvailability::Available;
        snapshot.system.media.can_next = true;
        snapshot.system.media.can_play = true;
        let mut document = FakeDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        let visible = |id: &str| document.visible.get(id).copied();
        assert_eq!(visible("media-next"), Some(true));
        assert_eq!(visible("media-next-disabled"), Some(false));
        assert_eq!(visible("media-playpause"), Some(true));
        assert_eq!(visible("media-playpause-disabled"), Some(false));
        assert_eq!(visible("media-previous"), Some(false));
        assert_eq!(visible("media-previous-disabled"), Some(true));
    }

    #[test]
    fn media_controls_all_disabled_without_player() {
        let snapshot = crate::ShellSnapshot::default();
        let mut document = FakeDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        for id in ["media-previous", "media-playpause", "media-next"] {
            assert_eq!(document.visible.get(id).copied(), Some(false), "{id}");
        }
        for id in [
            "media-previous-disabled",
            "media-playpause-disabled",
            "media-next-disabled",
        ] {
            assert_eq!(document.visible.get(id).copied(), Some(true), "{id}");
        }
    }
}
