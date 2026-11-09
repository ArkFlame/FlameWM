//! Weather tray button and popup projection (A09 part 2).
//!
//! Pure views ([`tray_view`], [`popup_view`]) hold every rule; the projection
//! functions only write node visibility and text. Icons are chosen from
//! [`WeatherCondition::icon_name`], one node per condition.

use super::*;
use crate::weather_host::{LocationSearch, SearchStatus};
use flamewm_weather::weather::{TemperatureUnit, WeatherCondition, WeatherSettings, WeatherState};
pub use flamewm_weather::weather_view::{
    condition_label, place_name, popup_view, tray_view, WEATHER_DAY_ROWS,
};

/// Every condition, one icon node each (`tray-weather-<suffix>`, `weather-icon-<suffix>`).
pub const WEATHER_CONDITIONS: [WeatherCondition; 9] = [
    WeatherCondition::Clear,
    WeatherCondition::FewClouds,
    WeatherCondition::Overcast,
    WeatherCondition::Fog,
    WeatherCondition::Drizzle,
    WeatherCondition::Rain,
    WeatherCondition::Snow,
    WeatherCondition::Storm,
    WeatherCondition::Unknown,
];

/// Icon node suffix, e.g. `Clear` -> `clear`, `Unknown` -> `none-available`.
#[must_use]
pub fn icon_suffix(condition: WeatherCondition) -> &'static str {
    condition
        .icon_name()
        .strip_prefix("weather-")
        .unwrap_or("none-available")
}

/// One icon node per condition under `prefix`; only `active` is visible, and only when `shown`.
fn set_icons(
    document: &mut impl UiDocumentAccess,
    prefix: &str,
    active: Option<WeatherCondition>,
    shown: bool,
) -> Result<(), String> {
    for condition in WEATHER_CONDITIONS {
        document.visible(
            &format!("{prefix}{}", icon_suffix(condition)),
            shown && active == Some(condition),
        )?;
    }
    Ok(())
}

/// Only the active icon carries a tint: its foreground is bound to the `--weather-*` variable
/// (colours live in `:root` of the shell sheet). `None` clears it, so the glyph inherits the bar colour.
fn set_icon_tints(
    document: &mut impl UiDocumentAccess,
    active: WeatherCondition,
    tint: Option<&'static str>,
) -> Result<(), String> {
    for condition in WEATHER_CONDITIONS {
        let id = format!("tray-weather-{}", icon_suffix(condition));
        match tint.filter(|_| condition == active) {
            Some(name) => document.foreground_variable(&id, name)?,
            None => document.foreground_clear(&id)?,
        }
    }
    Ok(())
}

/// Taskbar tray entry: visibility, one condition icon, and the temperature text under it.
/// `tint` is the `weatherTint` setting.
pub fn project_weather_tray(
    document: &mut impl UiDocumentAccess,
    state: &WeatherState,
    units: TemperatureUnit,
    shown: bool,
    tint: bool,
) -> Result<(), String> {
    let view = tray_view(state, units, shown, tint);
    document.visible("tray-weather", view.visible)?;
    super::project_tray_button(document, "tray-weather", view.visible)?;
    set_icons(
        document,
        "tray-weather-",
        Some(view.condition),
        view.visible,
    )?;
    set_icon_tints(document, view.condition, view.tint)?;
    document.text("tray-weather-temp", view.temperature)
}

/// Location picker rows shown in place of the forecast rows.
pub const WEATHER_RESULT_ROWS: usize = 5;

/// A snapshot shown from cache while the last refresh failed is dimmed.
#[must_use]
pub fn offline_dimmed(state: &WeatherState) -> bool {
    matches!(state, WeatherState::Offline(Some(_)))
}

/// The picker replaces the forecast rows while it has results or a status to show.
#[must_use]
pub fn picker_open(search: &LocationSearch) -> bool {
    !search.results.is_empty() || search.status != SearchStatus::Idle
}

fn search_status_text(status: SearchStatus) -> &'static str {
    match status {
        SearchStatus::Idle => "",
        SearchStatus::Searching => "Searching...",
        SearchStatus::NoMatches => "No matches",
        SearchStatus::Failed => "Search failed",
        SearchStatus::SaveFailed => "Could not save location",
    }
}

/// Weather popup body for every state, with the Open-Meteo attribution.
/// `veil` is the surface colour used to dim an offline snapshot.
pub fn project_weather_popup(
    document: &mut impl UiDocumentAccess,
    state: &WeatherState,
    settings: &WeatherSettings,
    search: &LocationSearch,
    veil: UiColor,
) -> Result<(), String> {
    let view = popup_view(state, settings);
    let picker = picker_open(search);
    document.text("weather-location", view.location)?;
    document.text("weather-temp", view.temperature)?;
    document.text_ref("weather-condition", condition_label(view.condition))?;
    document.text("weather-state", view.status)?;
    set_icons(document, "weather-icon-", Some(view.condition), true)?;
    for row in 1..=WEATHER_DAY_ROWS {
        let day = view.days.get(row - 1);
        let shown = day.is_some() && !picker;
        document.text_ref(
            &format!("weather-day-{row}-name"),
            day.map_or("", |d| d.weekday),
        )?;
        document.visible(&format!("weather-day-{row}-name"), shown)?;
        document.text_ref(
            &format!("weather-day-{row}-range"),
            day.map_or("", |d| d.range.as_str()),
        )?;
        document.visible(&format!("weather-day-{row}-range"), shown)?;
        set_icons(
            document,
            &format!("weather-day-{row}-icon-"),
            day.map(|d| d.condition),
            shown,
        )?;
    }
    document.visible("weather-offline-veil", offline_dimmed(state))?;
    document.background("weather-offline-veil", UiColor { a: 160, ..veil })?;
    document.text("weather-search-value", search.query.clone())?;
    document.visible("weather-search-placeholder", search.query.is_empty())?;
    for row in 1..=WEATHER_RESULT_ROWS {
        let location = search.results.get(row - 1).filter(|_| picker);
        document.text(
            &format!("weather-result-{row}"),
            location.map_or_else(String::new, |l| place_name(&l.name, l.country.as_deref())),
        )?;
        document.visible(&format!("weather-result-{row}"), location.is_some())?;
    }
    let status = if picker && search.results.is_empty() {
        search_status_text(search.status)
    } else {
        ""
    };
    document.text("weather-search-status", status.to_owned())?;
    document.visible("weather-search-status", !status.is_empty())?;
    document.text_ref(
        "weather-attribution",
        crate::weather_provider::OPEN_METEO_ATTRIBUTION,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_weather::weather::{DayForecast, WeatherError, WeatherLocation, WeatherSnapshot};
    use flamewm_weather::weather_view::weekday_label;

    fn berlin() -> WeatherLocation {
        WeatherLocation::new("Berlin", 52.52, 13.41, Some("Germany")).unwrap()
    }

    fn settings(units: TemperatureUnit) -> WeatherSettings {
        WeatherSettings {
            enabled: true,
            location: Some(berlin()),
            units,
        }
    }

    fn snapshot(days: usize) -> WeatherSnapshot {
        WeatherSnapshot {
            location: berlin(),
            temperature_c: 12,
            condition: WeatherCondition::Overcast,
            days: (0..days)
                .map(|i| DayForecast {
                    date: format!("2026-10-{:02}", 16 + i),
                    condition: WeatherCondition::Rain,
                    min_c: 3,
                    max_c: 14,
                })
                .collect(),
            is_day: true,
            fetched_at: 1,
        }
    }

    use flamewm_ui_x11::test_support::RecordingUiDoc as Recorder;

    #[test]
    fn every_condition_icon_resolves_to_a_breeze_asset() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../ui/shell/assets/web/breeze");
        for condition in WEATHER_CONDITIONS {
            let file = dir.join(format!("{}.svg", condition.icon_name()));
            assert!(file.is_file(), "missing icon asset {}", file.display());
        }
    }

    #[test]
    fn tray_visibility_follows_setting_not_fetch_state() {
        for state in [
            WeatherState::Unconfigured,
            WeatherState::Loading,
            WeatherState::Offline(None),
            WeatherState::Error(WeatherError::Transport),
        ] {
            let shown = tray_view(&state, TemperatureUnit::Celsius, true, true);
            assert!(shown.visible, "{state:?}");
            assert_eq!(shown.temperature, "--");
            assert_eq!(shown.condition, WeatherCondition::Unknown);
            assert!(
                !tray_view(&state, TemperatureUnit::Celsius, false, true).visible,
                "{state:?}"
            );
        }
    }

    #[test]
    fn tray_shows_integer_temperature_for_ready_and_offline_snapshots() {
        let ready = WeatherState::Ready(snapshot(0));
        assert_eq!(
            tray_view(&ready, TemperatureUnit::Celsius, true, true).temperature,
            "12\u{b0}"
        );
        assert_eq!(
            tray_view(&ready, TemperatureUnit::Fahrenheit, true, true).temperature,
            "54\u{b0}"
        );
        let offline = WeatherState::Offline(Some(snapshot(0)));
        assert_eq!(
            tray_view(&offline, TemperatureUnit::Celsius, true, true).condition,
            WeatherCondition::Overcast
        );
    }

    #[test]
    fn tray_projection_places_temperature_and_one_icon() {
        let mut doc = Recorder::default();
        project_weather_tray(
            &mut doc,
            &WeatherState::Ready(snapshot(0)),
            TemperatureUnit::Celsius,
            true,
            true,
        )
        .unwrap();
        assert_eq!(doc.texts["tray-weather-temp"], "12\u{b0}");
        assert!(doc.visible["tray-weather"]);
        assert!(doc.visible["tray-weather-overcast"]);
        assert!(!doc.visible["tray-weather-clear"]);

        let mut disabled = Recorder::default();
        project_weather_tray(
            &mut disabled,
            &WeatherState::Disabled,
            TemperatureUnit::Celsius,
            true,
            true,
        )
        .unwrap();
        assert!(!disabled.visible["tray-weather"]);
        assert!(!disabled.visible["tray-weather-none-available"]);

        let mut unconfigured = Recorder::default();
        project_weather_tray(
            &mut unconfigured,
            &WeatherState::Unconfigured,
            TemperatureUnit::Celsius,
            true,
            true,
        )
        .unwrap();
        assert!(unconfigured.visible["tray-weather"]);
        assert!(unconfigured.visible["tray-weather-none-available"]);
        assert_eq!(unconfigured.texts["tray-weather-temp"], "--");

        let mut off = Recorder::default();
        project_weather_tray(
            &mut off,
            &WeatherState::Disabled,
            TemperatureUnit::Celsius,
            false,
            true,
        )
        .unwrap();
        assert!(!off.visible["tray-weather"]);
        assert!(!off.visible["tray-weather-overcast"]);
    }

    #[test]
    fn popup_states_are_explicit_and_never_invent_values() {
        let s = settings(TemperatureUnit::Celsius);
        let cases = [
            (WeatherState::Disabled, "Weather is off"),
            (WeatherState::Unconfigured, "Choose a location"),
            (WeatherState::Loading, "Loading..."),
            (WeatherState::Offline(None), "Offline"),
            (
                WeatherState::Error(WeatherError::Parse("x")),
                "Could not load weather",
            ),
        ];
        for (state, status) in cases {
            let view = popup_view(&state, &s);
            assert_eq!(view.status, status, "{state:?}");
            assert_eq!(view.temperature, "--", "{state:?}");
            assert!(view.days.is_empty(), "{state:?}");
        }
        let ready = popup_view(&WeatherState::Ready(snapshot(3)), &s);
        assert_eq!(ready.status, "");
        assert_eq!(ready.location, "Berlin, Germany");
        assert_eq!(ready.days.len(), 3);
        let offline = popup_view(&WeatherState::Offline(Some(snapshot(1))), &s);
        assert_eq!(offline.status, "Offline");
        assert_eq!(offline.temperature, "12\u{b0}");
    }

    #[test]
    fn popup_caps_forecast_rows_at_seven_and_formats_ranges() {
        let view = popup_view(
            &WeatherState::Ready(snapshot(9)),
            &settings(TemperatureUnit::Fahrenheit),
        );
        assert_eq!(view.days.len(), WEATHER_DAY_ROWS);
        assert_eq!(view.days[0].range, "37 / 57");
        assert_eq!(view.days[0].weekday, "Fri");
    }

    #[test]
    fn popup_projection_hides_unused_rows_and_shows_attribution() {
        let mut doc = Recorder::default();
        project_weather_popup(
            &mut doc,
            &WeatherState::Ready(snapshot(2)),
            &settings(TemperatureUnit::Celsius),
            &LocationSearch::default(),
            UiColor {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
        )
        .unwrap();
        assert_eq!(doc.texts["weather-day-1-name"], "Fri");
        assert!(doc.visible["weather-day-1-icon-showers"]);
        assert_eq!(doc.texts["weather-day-3-name"], "");
        assert!(!doc.visible["weather-day-3-icon-showers"]);
        assert_eq!(
            doc.texts["weather-attribution"],
            crate::weather_provider::OPEN_METEO_ATTRIBUTION
        );
        assert_eq!(doc.texts["weather-state"], "");
    }

    #[test]
    fn picker_replaces_forecast_rows_and_offline_snapshot_is_veiled() {
        let mut search = LocationSearch::default();
        search.results = vec![berlin()];
        let veil = UiColor {
            r: 1,
            g: 2,
            b: 3,
            a: 255,
        };
        let mut doc = Recorder::default();
        project_weather_popup(
            &mut doc,
            &WeatherState::Offline(Some(snapshot(2))),
            &settings(TemperatureUnit::Celsius),
            &search,
            veil,
        )
        .unwrap();
        assert!(!doc.visible["weather-day-1-name"]);
        assert_eq!(doc.texts["weather-result-1"], "Berlin, Germany");
        assert!(doc.visible["weather-result-1"]);
        assert!(!doc.visible["weather-result-2"]);
        assert!(doc.visible["weather-offline-veil"]);
        assert_eq!(doc.backgrounds["weather-offline-veil"].a, 160);
        assert!(!doc.visible["weather-search-status"]);

        let mut searching = LocationSearch::default();
        searching.status = SearchStatus::Searching;
        let mut busy = Recorder::default();
        project_weather_popup(
            &mut busy,
            &WeatherState::Ready(snapshot(2)),
            &settings(TemperatureUnit::Celsius),
            &searching,
            veil,
        )
        .unwrap();
        assert_eq!(busy.texts["weather-search-status"], "Searching...");
        assert!(busy.visible["weather-search-status"]);
        assert!(!busy.visible["weather-offline-veil"]);
        assert!(!busy.visible["weather-day-1-name"]);
    }

    #[test]
    fn weekday_matches_known_dates() {
        assert_eq!(weekday_label("2026-10-16"), Some("Fri"));
        assert_eq!(weekday_label("2000-02-29"), Some("Tue"));
        assert_eq!(weekday_label("2026-13-01"), None);
        assert_eq!(weekday_label("nope"), None);
    }
}

#[cfg(test)]
mod panel_tray_tests {
    use super::*;
    use flamewm_ui_x11::test_support::RecordingUiDoc;
    use flamewm_weather::weather::{WeatherLocation, WeatherSnapshot};

    const PANEL_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr"));

    fn ready_21c() -> WeatherState {
        WeatherState::Ready(WeatherSnapshot {
            location: WeatherLocation::new("Berlin", 52.52, 13.41, Some("Germany")).unwrap(),
            temperature_c: 21,
            condition: WeatherCondition::Clear,
            days: Vec::new(),
            is_day: true,
            fetched_at: 1,
        })
    }

    const PANEL_VERTICAL_ARTIFACT: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel-vertical.rwr"));

    // OBSERVABLE, per compiled panel (horizontal 1080x72, vertical 44x1080 CSS viewport): the
    // temperature text is written and its pill has non-zero size; the visible glyph is centred on
    // the entry (centre y equal within 0.01); the pill top is at or below the glyph bottom (zero
    // overlap); the pill bottom sits exactly 1px above the entry bottom.
    // MUTATION: restoring the 8px margin-top, a 17px glyph, or dropping the vertical overlay
    // wrapper (pill in flow under the glyph) fails here.
    fn assert_weather_entry_clears_glyph(
        artifact: &'static [u8],
        viewport: (f32, f32),
        panel: &str,
    ) {
        let cases = [
            (
                WeatherState::Unconfigured,
                "--",
                "tray-weather-none-available",
            ),
            (ready_21c(), "21\u{b0}", "tray-weather-clear"),
        ];
        for (state, expected, glyph_id) in cases {
            let mut recorder = RecordingUiDoc::default();
            project_weather_tray(&mut recorder, &state, TemperatureUnit::Celsius, true, true)
                .unwrap();
            assert_eq!(recorder.texts["tray-weather-temp"], expected);

            let mut doc = flamewm_ui_x11::decode_document(artifact).expect("panel decodes");
            project_weather_tray(&mut doc, &state, TemperatureUnit::Celsius, true, true)
                .unwrap_or_else(|error| panic!("{panel} tray projection refused: {error}"));
            let button = doc
                .layout_box_by_id("tray-weather", viewport)
                .expect("button");
            let span = doc
                .layout_box_by_id("tray-weather-temp", viewport)
                .expect("span");
            let glyph = doc.layout_box_by_id(glyph_id, viewport).expect("glyph");
            assert!(
                span.2 > 0.0 && span.3 > 0.0,
                "{panel}: zero-size pill {span:?}"
            );
            let inside = span.0 >= button.0
                && span.1 >= button.1
                && span.0 + span.2 <= button.0 + button.2 + 0.5
                && span.1 + span.3 <= button.1 + button.3 + 0.5;
            assert!(inside, "{panel}: pill {span:?} outside entry {button:?}");
            let glyph_centre = glyph.1 + glyph.3 / 2.0;
            let entry_centre = button.1 + button.3 / 2.0;
            assert!(
                (glyph_centre - entry_centre).abs() < 0.01,
                "{panel}: glyph centre {glyph_centre} != entry centre {entry_centre} ({glyph:?} in {button:?})"
            );
            let glyph_bottom = glyph.1 + glyph.3;
            assert!(
                span.1 >= glyph_bottom - 0.01,
                "{panel}: pill top {} overlaps glyph bottom {glyph_bottom} ({span:?} vs {glyph:?})",
                span.1
            );
            let entry_bottom = button.1 + button.3;
            assert!(
                (span.1 + span.3 - (entry_bottom - 1.0)).abs() < 0.5,
                "{panel}: pill bottom {} not 1px above entry bottom {entry_bottom}",
                span.1 + span.3
            );
        }
    }

    // REGRESSION: the tray temperature text must be written into the compiled panel templates and
    // lay out clear of the weather glyph in both horizontal and vertical panels.
    #[test]
    fn tray_temperature_clears_glyph_in_horizontal_and_vertical_panels() {
        assert_weather_entry_clears_glyph(PANEL_ARTIFACT, (1080.0, 72.0), "horizontal");
        assert_weather_entry_clears_glyph(PANEL_VERTICAL_ARTIFACT, (44.0, 1080.0), "vertical");
    }

    // OBSERVABLE: both compiled panels paint a full RGBA frame through the software chain with the
    // weather entry projected. Set FLAMEWM_WEATHER_SHOT_DIR to dump `<panel>.rgba` for inspection.
    #[test]
    fn tray_weather_entry_paints_in_horizontal_and_vertical_panels() {
        let dump_dir = std::env::var("FLAMEWM_WEATHER_SHOT_DIR").ok();
        for (panel, artifact, viewport) in [
            ("horizontal", PANEL_ARTIFACT, (1080.0_f32, 72.0_f32)),
            ("vertical", PANEL_VERTICAL_ARTIFACT, (44.0_f32, 1080.0_f32)),
        ] {
            let mut doc = flamewm_ui_x11::decode_document(artifact).expect("panel decodes");
            project_weather_tray(&mut doc, &ready_21c(), TemperatureUnit::Celsius, true, true)
                .unwrap_or_else(|error| panic!("{panel} tray projection refused: {error}"));
            let pixels = flamewm_ui_x11::test_support::paint_popup_software(
                doc,
                viewport,
                [30, 40, 60, 255],
            );
            let expected = viewport.0 as usize * viewport.1 as usize * 4;
            assert_eq!(pixels.len(), expected, "{panel} frame size");
            if let Some(dir) = &dump_dir {
                std::fs::write(format!("{dir}/{panel}.rgba"), &pixels).expect("write frame");
            }
        }
    }

    // OBSERVABLE: with tinting on only the active condition glyph binds its `--weather-*`
    // variable; tinting off and unknown data bind nothing (they clear the foreground instead).
    // MUTATION: tinting every icon, or tinting Unknown, fails here.
    #[test]
    fn tint_is_written_to_the_active_glyph_only() {
        let rain = WeatherState::Ready(WeatherSnapshot {
            condition: WeatherCondition::Rain,
            ..match ready_21c() {
                WeatherState::Ready(snapshot) => snapshot,
                _ => unreachable!(),
            }
        });
        let mut on = flamewm_ui_x11::test_support::RecordingUiDoc::default();
        project_weather_tray(&mut on, &rain, TemperatureUnit::Celsius, true, true).unwrap();
        assert_eq!(
            on.foreground_variables.keys().collect::<Vec<_>>(),
            vec!["tray-weather-showers"]
        );
        assert_eq!(
            on.foreground_variables["tray-weather-showers"],
            "--weather-rain"
        );
        assert!(on.foregrounds.is_empty());

        let mut off = flamewm_ui_x11::test_support::RecordingUiDoc::default();
        project_weather_tray(&mut off, &rain, TemperatureUnit::Celsius, true, false).unwrap();
        assert!(off.foreground_variables.is_empty());
        assert!(off.foregrounds.is_empty());

        let mut unknown = flamewm_ui_x11::test_support::RecordingUiDoc::default();
        project_weather_tray(
            &mut unknown,
            &WeatherState::Unconfigured,
            TemperatureUnit::Celsius,
            true,
            true,
        )
        .unwrap();
        assert!(unknown.foregrounds.is_empty());
    }

    // REGRESSION: the condition glyph stays centred on the taskbar entry; the temperature pill
    // overlays the glyph and sits 4 px below that centre (`.tray-weather-temp` margin-top 8px).
    // Measured in the real compiled panel.
    // OBSERVABLE: |glyph centre - entry centre| <= 0.5px; pill centre - entry centre == 4px +-0.5.
    // MUTATION: restoring the stacked column, or dropping the offset, fails here.
    #[test]
    fn tray_glyph_and_temperature_share_the_entry_vertical_centre() {
        let mut doc = flamewm_ui_x11::decode_document(PANEL_ARTIFACT).expect("panel decodes");
        project_weather_tray(&mut doc, &ready_21c(), TemperatureUnit::Celsius, true, true)
            .expect("tray projection");
        let viewport = (1080.0, 72.0);
        let button = doc
            .layout_box_by_id("tray-weather", viewport)
            .expect("button");
        let glyph = doc
            .layout_box_by_id("tray-weather-clear", viewport)
            .expect("glyph");
        let pill = doc
            .layout_box_by_id("tray-weather-temp", viewport)
            .expect("pill");
        let centre_y = button.1 + button.3 / 2.0;
        let glyph_dy = glyph.1 + glyph.3 / 2.0 - centre_y;
        assert!(
            glyph_dy.abs() <= 0.01,
            "glyph {glyph:?} off centre {button:?}"
        );
        // The pill sits in the lower part of the entry, below the glyph, 1px above its bottom.
        assert!(
            pill.1 >= glyph.1 + glyph.3 - 0.01,
            "pill {pill:?} overlaps glyph {glyph:?}"
        );
        assert!(
            (pill.1 + pill.3 - (button.1 + button.3 - 1.0)).abs() <= 0.5,
            "pill {pill:?} not anchored 1px above entry bottom {button:?}"
        );
        assert!(pill.2 > 0.0 && pill.3 > 0.0, "zero-size pill {pill:?}");
    }
}
