use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_compiler::{CompileOptions, CompileOutput, compile_file, decode, encode};

#[path = "ui_contract.rs"]
mod ui_contract;

const DESKTOP_ASSETS: [&str; 2] = ["wallpaper.ppm", "watermark.png"];

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("ui_contract.rs").display()
    );
    let html_path = manifest.join("../../ui/desktop/index.html");
    let css_path = manifest.join("../../ui/desktop/desktop.css");
    let menu_css_path = manifest.join("../../ui/shared/menu.css");
    let raster = root.join("assets/raster");
    let wallpaper = root.join("assets/wallpapers/fitz-roy-dawn.webp");
    let breeze = root.join("assets/web/breeze");
    println!("cargo:rerun-if-changed={}", html_path.display());
    println!("cargo:rerun-if-changed={}", css_path.display());
    println!("cargo:rerun-if-changed={}", menu_css_path.display());
    rerun_tree(&breeze);
    println!("cargo:rerun-if-changed={}", wallpaper.display());
    assert!(
        wallpaper.is_file(),
        "required packaged default wallpaper is missing: {}",
        wallpaper.display()
    );
    for asset in DESKTOP_ASSETS {
        println!("cargo:rerun-if-changed={}", raster.join(asset).display());
    }

    let html = fs::read_to_string(&html_path).expect("read desktop UI");
    let items = (0..128)
        .map(|index| {
            format!(
                "<button id=\"desktop-item-{index}\" class=\"desktop-item\" data-action=\"desktop.item.{index}\">\
<img id=\"desktop-glyph-{index}-desktop-launcher\" class=\"desktop-glyph\" src=\"assets/breeze/internet-web-browser.svg\">\
<img id=\"desktop-glyph-{index}-file\" class=\"desktop-glyph\" src=\"assets/breeze/folder-documents.svg\">\
<img id=\"desktop-glyph-{index}-symlink\" class=\"desktop-glyph\" src=\"assets/breeze/folder.svg\">\
<img id=\"desktop-glyph-{index}-trash\" class=\"desktop-glyph\" src=\"assets/breeze/user-trash.svg\">\
<img id=\"desktop-glyph-{index}-directory\" class=\"desktop-glyph\" src=\"assets/breeze/folder.svg\">\
<div id=\"desktop-label-{index}\" class=\"desktop-label\"><div id=\"desktop-label-{index}-line-1\" class=\"desktop-label-line\">&#8203;</div><div id=\"desktop-label-{index}-line-2\" class=\"desktop-label-line\">&#8203;</div></div>\
</button>"
            )
        })
        .collect::<Vec<_>>()
        .join("\n  ");
    let html = html
        .replace("<!--ITEMS-->", &items)
        .replace("<!--WIDGETS-->", &widget_slots())
        .replace("<!--WIDGET-PANELS-->", &widget_panels());

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let staged_ui = out.join("desktop");
    let staged_assets = staged_ui.join("assets");
    fs::create_dir_all(&staged_assets).expect("create staged desktop assets");
    fs::write(staged_ui.join("index.html"), html).expect("write staged desktop UI");
    for asset in DESKTOP_ASSETS {
        fs::copy(raster.join(asset), staged_assets.join(asset)).expect("stage desktop asset");
    }
    stage_tree(&breeze, &staged_assets.join("breeze"));
    // These three local menu glyphs are not part of the selectable system icon
    // themes. Always bundle them with the desktop RWR so the same UI is shown
    // with MacTahoe, Fluent and a minimal installed asset directory.
    let menu_icons = root.join("ui/desktop/icons");
    for icon in [
        "flamewm-convert.svg",
        "flamewm-add-workspace.svg",
        "flamewm-new-folder.svg",
    ] {
        let source = menu_icons.join(icon);
        println!("cargo:rerun-if-changed={}", source.display());
        fs::copy(source, staged_assets.join("breeze").join(icon))
            .expect("stage FlameWM-owned menu icon");
    }
    let staged_wallpapers = staged_assets.join("wallpapers");
    fs::create_dir_all(&staged_wallpapers).expect("create staged wallpaper dir");
    fs::copy(&wallpaper, staged_wallpapers.join("fitz-roy-dawn.webp"))
        .expect("stage default wallpaper");

    let output = compile_file(
        &staged_ui.join("index.html"),
        &[css_path, menu_css_path],
        CompileOptions::default(),
    )
    .expect("compile desktop UI");
    validate_text_targets(&output);
    let compiled_ids = output
        .document
        .nodes
        .iter()
        .map(|node| &*node.id)
        .collect::<Vec<_>>();
    validate_compiled_desktop_nodes(&compiled_ids);
    let bytes = encode(&output.document).expect("encode desktop UI");
    // This is the exact serialization boundary that the runtime reads via
    // include_bytes!(OUT_DIR/flamewm-desktop.rwr). HTML text is not proof.
    let decoded = decode(&bytes).expect("decode encoded desktop RWR");
    let decoded_ids = decoded
        .nodes
        .iter()
        .map(|node| &*node.id)
        .collect::<Vec<_>>();
    validate_compiled_desktop_nodes(&decoded_ids);
    fs::write(out.join("flamewm-desktop.rwr"), bytes).expect("write validated desktop UI");
}

const WIDGET_SLOTS: usize = 8;
const WIDGET_LINES: usize = 6;
const WIDGET_SPANS: usize = 6;
const WIDGET_CONFIG_ROWS: usize = 18;
const WEEKDAY_INITIALS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
const WIDGET_KINDS: [(&str, &str, &str); 10] = [
    (
        "weather",
        "Weather",
        "City, current temperature and the next six hours.",
    ),
    (
        "calendar",
        "Calendar month",
        "Current month with today marked.",
    ),
    (
        "clock",
        "Time and date",
        "Clock with weekday and date, side by side or stacked.",
    ),
    ("cpu", "CPU usage", "Processor load as a circular gauge."),
    ("memory", "Memory usage", "RAM in use as a circular gauge."),
    (
        "history",
        "CPU and RAM graph",
        "Rolling CPU and memory history as a sparkline.",
    ),
    (
        "network",
        "Network traffic",
        "Download and upload rates as a sparkline.",
    ),
    (
        "ring",
        "Display ring",
        "Circular gauge for CPU or RAM, with a percentage.",
    ),
    (
        "media",
        "Media player",
        "Now playing, with previous, play and next.",
    ),
    (
        "custom",
        "Custom text",
        "Your own text with %placeholders% and <color> tags.",
    ),
];

fn aligned_line(id: &str, class: &str, content: &str) -> String {
    format!(
        "<div id=\"{id}\" class=\"widget-line {class}\"><div id=\"{id}-lead\" class=\"widget-spacer\"></div>{content}<div id=\"{id}-trail\" class=\"widget-spacer\"></div></div>"
    )
}

/// Grid-overlay capacity: 32 columns by 18 rows. Must match `WIDGET_OVERLAY_CELLS` in `widgets.rs`.
const WIDGET_OVERLAY_CELLS: usize = 32 * 18;

fn widget_slots() -> String {
    (1..=WIDGET_SLOTS)
        .map(|n| {
            let w = format!("widget-{n}");
            let clock = format!(
                "<div id=\"{w}-clock\" class=\"widget-clock\"><div id=\"{w}-clock-stack\" class=\"widget-clock-stack\">{}{}</div>{}</div>",
                aligned_line(
                    &format!("{w}-clock-time-line"),
                    "",
                    &format!("<div id=\"{w}-clock-time\" class=\"widget-clock-time\">&#8203;</div>")
                ),
                aligned_line(
                    &format!("{w}-clock-date-line"),
                    "",
                    &format!("<div id=\"{w}-clock-date\" class=\"widget-clock-date\">&#8203;</div>")
                ),
                aligned_line(
                    &format!("{w}-clock-row"),
                    "widget-clock-row",
                    &format!(
                        "<div id=\"{w}-clock-row-time\" class=\"widget-clock-time\">&#8203;</div><div id=\"{w}-clock-row-date\" class=\"widget-clock-date widget-clock-row-date\">&#8203;</div>"
                    )
                ),
            );
            let numerals: String = (1..=12)
                .map(|n| {
                    format!(
                        "<div id=\"{w}-analog-n{n}\" class=\"widget-analog-n widget-analog-n{n}\">{n}</div>"
                    )
                })
                .collect();
            let analog = format!(
                "<div id=\"{w}-analog\" class=\"widget-analog\" hidden><img id=\"{w}-analog-image\" class=\"widget-analog-image\" src=\"assets/breeze/folder.svg\">{numerals}</div>"
            );
            let gauge = format!(
                "<div id=\"{w}-gauge\" class=\"widget-gauge\">\
<div id=\"{w}-gauge-main\" class=\"widget-gauge-main\">\
<div id=\"{w}-gauge-ring\" class=\"widget-gauge-ring\"><img id=\"{w}-gauge-image\" class=\"widget-gauge-image\" src=\"assets/breeze/folder.svg\">\
<div id=\"{w}-gauge-center\" class=\"widget-gauge-center\"><div id=\"{w}-gauge-percent\" class=\"widget-gauge-percent\">&#8203;</div></div></div>\
<div id=\"{w}-gauge-copy\" class=\"widget-gauge-copy\"><div id=\"{w}-gauge-title\" class=\"widget-gauge-title\">&#8203;</div>\
<div id=\"{w}-gauge-value\" class=\"widget-gauge-value\">&#8203;</div><div id=\"{w}-gauge-detail\" class=\"widget-gauge-detail\">&#8203;</div></div>\
</div><div id=\"{w}-gauge-caption\" class=\"widget-gauge-caption\">&#8203;</div></div>"
            );
            let history = format!(
                "<div id=\"{w}-history\" class=\"widget-history\" hidden><img id=\"{w}-history-image\" class=\"widget-history-image\" src=\"assets/breeze/folder.svg\"></div>"
            );
            let network = format!(
                "<div id=\"{w}-network\" class=\"widget-history\" hidden><img id=\"{w}-network-image\" class=\"widget-history-image\" src=\"assets/breeze/folder.svg\"></div>"
            );
            let media = format!(
                "<div id=\"{w}-media\" class=\"widget-media\">\
<div id=\"{w}-media-head\" class=\"widget-media-head\">\
<div id=\"{w}-media-art\" class=\"widget-media-art\"><img src=\"assets/breeze/folder-music.svg\"></div>\
<div id=\"{w}-media-text\" class=\"widget-media-text\">\
<div id=\"{w}-media-player\" class=\"widget-media-player\">&#8203;</div>\
<div id=\"{w}-media-title\" class=\"widget-media-title\">&#8203;</div>\
<div id=\"{w}-media-artist\" class=\"widget-media-artist\">&#8203;</div>\
</div></div>\
<div id=\"{w}-media-controls\" class=\"widget-media-controls\">\
<button id=\"{w}-media-prev\" class=\"widget-media-button\" data-action=\"widget.{n}.media.prev\"><img src=\"assets/breeze/media-skip-backward.svg\"></button>\
<button id=\"{w}-media-play\" class=\"widget-media-button\" data-action=\"widget.{n}.media.play\"><img id=\"{w}-media-play-icon\" src=\"assets/breeze/media-playback-start.svg\"><img id=\"{w}-media-pause-icon\" src=\"assets/breeze/media-playback-pause.svg\"></button>\
<button id=\"{w}-media-next\" class=\"widget-media-button\" data-action=\"widget.{n}.media.next\"><img src=\"assets/breeze/media-skip-forward.svg\"></button>\
</div></div>"
            );
            let hours: String = (1..=6)
                .map(|h| {
                    format!(
                        "<div id=\"{w}-weather-hour-{h}\" class=\"widget-weather-hour\"><div id=\"{w}-weather-hour-{h}-time\" class=\"widget-weather-hour-time\">&#8203;</div><div id=\"{w}-weather-hour-{h}-temp\" class=\"widget-weather-hour-temp\">&#8203;</div></div>"
                    )
                })
                .collect();
            let weather = format!(
                "<div id=\"{w}-weather\" class=\"widget-weather\" hidden>\
<div id=\"{w}-weather-head\" class=\"widget-weather-head\">\
<div id=\"{w}-weather-left\" class=\"widget-weather-left\"><div id=\"{w}-weather-city\" class=\"widget-weather-city\">&#8203;</div><div id=\"{w}-weather-temp\" class=\"widget-weather-temp\">&#8203;</div></div>\
<div id=\"{w}-weather-right\" class=\"widget-weather-right\"><div id=\"{w}-weather-condition\" class=\"widget-weather-condition\">&#8203;</div><div id=\"{w}-weather-range\" class=\"widget-weather-range\">&#8203;</div></div>\
</div><div id=\"{w}-weather-hours\" class=\"widget-weather-hours\">{hours}</div>\
<div id=\"{w}-weather-attr\" class=\"widget-weather-attr\" hidden>&#8203;</div></div>"
            );
            let weekdays: String = WEEKDAY_INITIALS
                .iter()
                .enumerate()
                .map(|(c, initial)| {
                    format!("<div id=\"{w}-cal-wd-{c}\" class=\"widget-cal-cell widget-cal-wd\">{initial}</div>")
                })
                .collect();
            let weeks: String = (0..6)
                .map(|r| {
                    let cells: String = (0..7)
                        .map(|c| {
                            format!(
                                "<div id=\"{w}-cal-{r}-{c}\" class=\"widget-cal-cell\">&#8203;</div>"
                            )
                        })
                        .collect();
                    format!("<div class=\"widget-cal-row\">{cells}</div>")
                })
                .collect();
            let calendar = format!(
                "<div id=\"{w}-calendar\" class=\"widget-calendar\" hidden>\
<div id=\"{w}-cal-month\" class=\"widget-cal-month\">&#8203;</div>\
<div class=\"widget-cal-row\">{weekdays}</div>{weeks}</div>"
            );
            let lines = (1..=WIDGET_LINES)
                .map(|l| {
                    let spans = (1..=WIDGET_SPANS)
                        .map(|s| {
                            format!(
                                "<div id=\"{w}-line-{l}-span-{s}\" class=\"widget-span\">&#8203;</div>"
                            )
                        })
                        .collect::<String>();
                    aligned_line(&format!("{w}-line-{l}"), "", &spans)
                })
                .collect::<String>();
            let custom = format!(
                "<div id=\"{w}-custom\" class=\"widget-custom\">{lines}</div><div id=\"{w}-editor\" class=\"widget-editor\">&#8203;</div>"
            );
            // Border and corner bands resize the widget; they sit after the content so they
            // win hit-testing inside the card.
            let edges: String = ["n", "s", "e", "w", "ne", "nw", "se", "sw"]
                .iter()
                .map(|edge| {
                    format!(
                        "<div id=\"{w}-edge-{edge}\" class=\"widget-edge widget-edge-{edge}\" data-action=\"widget.{n}.resize-{edge}\"></div>"
                    )
                })
                .collect();
            format!(
                "<div id=\"{w}\" class=\"widget\" data-action=\"widget.{n}.move\" hidden>{clock}{analog}{gauge}{history}{network}{media}{weather}{calendar}{custom}{edges}</div>"
            )
        })
        .collect::<Vec<_>>()
        .join("\n  ")
        // Slot overlay pool: one hidden outline per possible grid cell (up to 32x18),
        // shown only while a widget is being dragged or resized. Emitted before the
        // ghost so the ghost paints above the outlines in the shared overlay layer.
        + "\n  "
        + &(0..WIDGET_OVERLAY_CELLS)
            .map(|index| {
                format!("<div id=\"widget-slot-{index}\" class=\"widget-slot-cell\" hidden></div>")
            })
            .collect::<Vec<_>>()
            .join("")
        // Drag ghost: the target cell outline shown while a widget is moved or resized.
        + "\n  <div id=\"widget-ghost\" class=\"widget-ghost\" hidden></div>"
}

fn widget_card_preview(kind: &str) -> String {
    match kind {
        "clock" => "<div class=\"widget-card-preview-clock\"><div id=\"widget-card-clock-time\" class=\"widget-card-big\">10:02</div><div id=\"widget-card-clock-date\" class=\"widget-card-small\">Wednesday, 29 June</div></div>".to_owned(),
        "cpu" | "memory" => format!(
            "<img id=\"widget-card-{kind}-image\" class=\"widget-card-ring\" src=\"assets/breeze/folder.svg\">"
        ),
        "weather" => "<div class=\"widget-card-preview-weather\"><div class=\"widget-card-small\">Weather</div><div class=\"widget-card-big\">--&#176;</div></div>".to_owned(),
        "calendar" => "<div class=\"widget-card-preview-calendar\"><div class=\"widget-card-small\">MONTH</div><div class=\"widget-card-small\">S M T W T F S</div></div>".to_owned(),
        "media" => "<div class=\"widget-card-preview-media\"><div class=\"widget-card-small\">Now playing</div><div class=\"widget-card-medium\">Song title</div><div class=\"widget-card-icons\"><img src=\"assets/breeze/media-skip-backward.svg\"><img src=\"assets/breeze/media-playback-start.svg\"><img src=\"assets/breeze/media-skip-forward.svg\"></div></div>".to_owned(),
        _ => "<div class=\"widget-card-preview-custom\"><div class=\"widget-card-markup\"><div class=\"widget-card-chip widget-card-chip-red\">Welcome,</div><div class=\"widget-card-chip widget-card-chip-blue\">%username%</div></div><div class=\"widget-card-small\">%date% %time%</div></div>".to_owned(),
    }
}

fn widget_panels() -> String {
    let cards = WIDGET_KINDS
        .iter()
        .map(|(kind, title, text)| {
            let text = text.replace('<', "&lt;").replace('>', "&gt;");
            format!(
                "<div id=\"widget-card-{kind}\" class=\"widget-card\"><div class=\"widget-card-preview\">{}</div>\
<div class=\"widget-card-copy\"><div class=\"widget-card-title\">{title}</div><div class=\"widget-card-text\">{text}</div></div>\
<button id=\"widget-card-{kind}-add\" class=\"widget-card-add\" data-action=\"widget.gallery.add.{kind}\">Add</button></div>",
                widget_card_preview(kind)
            )
        })
        .collect::<String>();
    let rows = (0..WIDGET_CONFIG_ROWS)
        .map(|r| {
            format!(
                "<div id=\"widget-config-row-{r}\" class=\"widget-config-row\"><div id=\"widget-config-row-{r}-label\" class=\"widget-config-label\">&#8203;</div>\
<div id=\"widget-config-row-{r}-swatch\" class=\"widget-config-swatch\"></div>\
<button id=\"widget-config-row-{r}-down\" class=\"widget-config-step\" data-action=\"widget.config.{r}.down\">&lt;</button>\
<div id=\"widget-config-row-{r}-value\" class=\"widget-config-value\">&#8203;</div>\
<button id=\"widget-config-row-{r}-up\" class=\"widget-config-step\" data-action=\"widget.config.{r}.up\">&gt;</button></div>"
            )
        })
        .collect::<String>();
    format!(
        "<section id=\"widget-menu\" class=\"sticky-menu context-menu\" data-action=\"widget.menu.surface\">\
<button id=\"widget-row-configure\" class=\"menu-row\" data-action=\"widget.menu.configure\"><img src=\"assets/breeze/configure.svg\"><span>Configure Widget</span></button>\
<button id=\"widget-row-edit\" class=\"menu-row\" data-action=\"widget.menu.edit\"><img src=\"assets/breeze/edit-rename.svg\"><span>Edit Text</span></button>\
<button id=\"widget-row-lock\" class=\"menu-row\" data-action=\"widget.menu.lock\"><img src=\"assets/breeze/system-lock-screen.svg\"><span id=\"widget-row-lock-label\">Lock Position</span></button>\
<button id=\"widget-row-gallery\" class=\"menu-row\" data-action=\"widget.menu.gallery\"><img src=\"assets/breeze/list-add.svg\"><span>Add Widgets</span></button>\
<button id=\"widget-row-remove\" class=\"menu-row\" data-action=\"widget.menu.remove\"><img src=\"assets/breeze/edit-delete.svg\"><span>Remove Widget</span></button>\
</section>\n  \
<section id=\"widget-gallery\" class=\"widget-gallery context-menu\" data-action=\"widget.gallery.surface\">\
<div class=\"widget-panel-header\"><div class=\"widget-panel-heading\"><div class=\"widget-panel-title\">Widgets</div><div id=\"widget-gallery-count\" class=\"widget-panel-subtitle\">&#8203;</div></div>\
<button id=\"widget-gallery-close\" class=\"widget-panel-close\" data-action=\"widget.gallery.close\">Done</button></div>{cards}</section>\n  \
<section id=\"widget-config\" class=\"widget-config context-menu\" data-action=\"widget.config.surface\">\
<div class=\"widget-panel-header\"><div class=\"widget-panel-heading\"><div id=\"widget-config-title\" class=\"widget-panel-title\">&#8203;</div><div class=\"widget-panel-subtitle\">Changes apply immediately</div></div>\
<button id=\"widget-config-close\" class=\"widget-panel-close\" data-action=\"widget.config.close\">Done</button></div>{rows}</section>"
    )
}

fn validate_compiled_desktop_nodes(ids: &[&str]) {
    ui_contract::require_desktop_nodes(|id| ids.contains(&id)).unwrap_or_else(|error| {
        panic!("{error}; verify ui/desktop/index.html and rebuild flamewm-desktop")
    });
    for required in ui_contract::REQUIRED_DESKTOP_IDS {
        assert_eq!(
            ids.iter().filter(|id| *id == required).count(),
            1,
            "desktop compiled RWR must contain exactly one node with id '{required}'"
        );
    }
}

fn stage_tree(source: &Path, dest: &Path) {
    fs::create_dir_all(dest).expect("create staged asset dir");
    for entry in fs::read_dir(source).expect("read staged asset dir") {
        let entry = entry.expect("read staged asset entry");
        let path = entry.path();
        if path.is_file() {
            fs::copy(&path, dest.join(entry.file_name())).expect("stage asset file");
        }
    }
}

fn validate_text_targets(output: &CompileOutput) {
    let mut ids = Vec::with_capacity(272);
    for slot in 1..=16 {
        ids.push(format!("sticky-{slot}-text"));
    }
    for index in 0..128 {
        ids.push(format!("desktop-label-{index}-line-1"));
        ids.push(format!("desktop-label-{index}-line-2"));
    }
    for slot in 0..8 {
        let id = format!("convert-slot-{slot}");
        assert_dynamic_text_target(output, &id);
    }
    assert_dynamic_text_target(output, "convert-wheel-hub-text");
    assert_dynamic_text_target(output, "confirm-text");
    for id in &ids {
        let position = output
            .document
            .nodes
            .iter()
            .position(|node| node.id == *id)
            .unwrap_or_else(|| panic!("desktop UI is missing runtime text target '{id}'"));
        let first = output.document.nodes[position]
            .first_child
            .unwrap_or_else(|| panic!("desktop UI text target '{id}' has no text child"));
        let child = &output.document.nodes[first as usize];
        if child.first_child.is_some()
            || child.next_sibling.is_some()
            || !child.text.contains('\u{200b}')
        {
            panic!("desktop UI text target '{id}' is not a single-text-node element");
        }
    }
}

/// Dynamic button labels/hub text must be a single text child; otherwise
/// `RuntimeDocument::set_text` fails at the first right-click during startup.
fn assert_dynamic_text_target(output: &CompileOutput, id: &str) {
    let node = output
        .document
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap_or_else(|| panic!("desktop UI is missing dynamic text target '{id}'"));
    let first = node
        .first_child
        .expect("dynamic text target requires one text child");
    let child = &output.document.nodes[first as usize];
    assert!(
        child.next_sibling.is_none() && child.first_child.is_none(),
        "dynamic text target '{id}' must contain one text child"
    );
}

fn rerun_tree(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files = fs::read_dir(dir)
        .expect("read tracked asset dir")
        .map(|entry| entry.expect("read tracked asset entry").path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
}
