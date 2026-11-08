//! Painting of the finder document: rows, tiles, preview, details and sheets.

use super::*;

impl Finder {
    pub(super) fn paint(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.paint_theme(doc)?;
        self.paint_query(doc)?;
        self.paint_section(doc)?;
        self.paint_items(doc)?;
        self.paint_status(doc)?;
        self.paint_overlays(doc)
    }

    pub(super) fn paint_theme(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = surface_fill(&self.theme);
        doc.background("finder", UiColor::from_rgb(surface.rgb, surface.alpha))?;
        flamewm_ui_x11::surface_style::project_surface_outline(doc, "finder", &self.theme)?;
        doc.color_variable("--accent", UiColor::from_rgb(self.palette.accent, 255))?;
        doc.color_variable("--text", UiColor::from_rgb(self.palette.text, 255))?;
        doc.color_variable("--muted", UiColor::from_rgb(self.palette.muted, 255))?;
        doc.color_variable("--subtle", UiColor::from_rgb(self.palette.subtle, 255))
    }

    pub(super) fn paint_query(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let typing = !self.query.is_empty();
        if typing {
            doc.text("query", self.query.clone())?;
            doc.foreground("query", UiColor::from_rgb(self.palette.text, 255))?;
        } else {
            doc.text("query", PLACEHOLDERS[self.placeholder].to_owned())?;
            doc.foreground("query", UiColor::from_rgb(self.palette.subtle, 255))?;
        }
        doc.visible("clear", typing)?;
        let items = self.items().len();
        let count = match palette::mode(&self.query) {
            _ if !typing => String::new(),
            Mode::Files => match self.hits.total + self.quick_count() {
                0 => "No results".to_owned(),
                1 => "1 result".to_owned(),
                total => format!("{} results", format::count(total)),
            },
            Mode::Commands(_) => match items {
                0 => "No commands".to_owned(),
                n => format!("{n} commands"),
            },
            Mode::Scopes(_) => match items {
                0 => "No folders".to_owned(),
                n => format!("{n} folders"),
            },
        };
        doc.text("count", count)?;
        doc.visible("count", typing)
    }

    pub(super) fn paint_section(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let label = match palette::mode(&self.query) {
            Mode::Files if self.query.is_empty() => "Recent",
            Mode::Files => "Results",
            Mode::Commands(_) => "Commands",
            Mode::Scopes(_) => "Folders",
        };
        doc.text("section", label.to_owned())?;
        doc.foreground("section", UiColor::from_rgb(self.palette.subtle, 255))
    }

    pub(super) fn paint_items(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let items = self.items();
        let tiles = self.tiles_shown();
        doc.visible("grid", tiles)?;
        doc.visible("list", !tiles)?;
        doc.visible("empty", items.is_empty())?;
        let empty_text = match palette::mode(&self.query) {
            _ if self.walking && self.view().is_empty() => "Indexing files...",
            Mode::Commands(_) => "No commands match",
            Mode::Scopes(_) => "No folders match",
            Mode::Files if self.query.is_empty() => "Nothing to show yet",
            Mode::Files => "No files match",
        };
        doc.text("empty-text", empty_text.to_owned())?;
        let now = now_secs();
        for slot in 0..ROW_SLOTS {
            let position = self.first + slot;
            let item = items.get(position);
            let selected = position == self.selected;
            if tiles {
                self.paint_tile(doc, slot, item, selected, now)?;
            } else {
                self.paint_row(doc, slot, item, selected, now)?;
            }
        }
        Ok(())
    }

    pub(super) fn paint_row(
        &self,
        doc: &mut impl UiDocumentAccess,
        slot: usize,
        item: Option<&Item>,
        selected: bool,
        now: i64,
    ) -> Result<(), String> {
        let row = format!("row-{slot}");
        let Some(item) = item else {
            return doc.visible(&row, false);
        };
        doc.visible(&row, true)?;
        doc.background(&row, UiColor::from_rgb(0x00ff_ffff, if selected { 36 } else { 0 }))?;
        let (kind, pieces, parent, meta) = match item {
            Item::Entry(position) => {
                let entry = self.view().entries[*position].clone();
                (
                    Kind::of(&entry),
                    search::segments(entry.name(), &self.parsed.terms),
                    format::parent_for_display(&entry.path, &self.home),
                    meta_line(&entry, now),
                )
            }
            Item::Command(spec) => (
                Kind::Code,
                vec![(spec.label.to_owned(), false)],
                String::new(),
                spec.hint.to_owned(),
            ),
            Item::Scope(choice) => (
                Kind::Folder,
                vec![(choice.label.clone(), false)],
                choice.query.trim().to_owned(),
                String::new(),
            ),
            Item::Calc(answer) => (
                Kind::Code,
                vec![(answer.result.clone(), false)],
                answer.input.clone(),
                "Calculator".to_owned(),
            ),
            Item::App(position) => {
                let Some(app) = self.apps.get(*position) else {
                    return doc.visible(&row, false);
                };
                let detail = if app.comment.is_empty() {
                    app.generic_name.clone()
                } else {
                    app.comment.clone()
                };
                (
                    Kind::Code,
                    search::segments(&app.name, &self.parsed.terms),
                    detail,
                    "Application".to_owned(),
                )
            }
            Item::Action(action) => (
                Kind::Code,
                vec![(action.label.to_owned(), false)],
                "System action".to_owned(),
                String::new(),
            ),
        };
        self.paint_icon(doc, &format!("ico-{slot}"), kind)?;
        self.paint_segments(doc, slot, pieces)?;
        doc.text(&format!("parent-{slot}"), parent)?;
        doc.text(&format!("meta-{slot}"), meta)
    }

    pub(super) fn paint_tile(
        &self,
        doc: &mut impl UiDocumentAccess,
        slot: usize,
        item: Option<&Item>,
        selected: bool,
        now: i64,
    ) -> Result<(), String> {
        let tile = format!("tile-{slot}");
        let Some(Item::Entry(position)) = item else {
            return doc.visible(&tile, false);
        };
        let entry = self.view().entries[*position].clone();
        doc.visible(&tile, true)?;
        doc.background(&tile, UiColor::from_rgb(0x00ff_ffff, if selected { 36 } else { 0 }))?;
        self.paint_icon(doc, &format!("tile-{slot}-ico"), Kind::of(&entry))?;
        doc.text_ref(&format!("tile-name-{slot}"), entry.name())?;
        doc.text(&format!("tile-meta-{slot}"), meta_line(&entry, now))
    }

    /// Shows the glyph for `kind` under `prefix` and hides the others.
    pub(super) fn paint_icon(
        &self,
        doc: &mut impl UiDocumentAccess,
        prefix: &str,
        kind: Kind,
    ) -> Result<(), String> {
        for candidate in KINDS {
            doc.visible(&format!("{prefix}-{}", candidate.slug()), candidate == kind)?;
        }
        Ok(())
    }

    pub(super) fn paint_segments(
        &self,
        doc: &mut impl UiDocumentAccess,
        slot: usize,
        pieces: Vec<(String, bool)>,
    ) -> Result<(), String> {
        let pieces = fold_pieces(pieces, SEGMENT_SLOTS);
        for segment in 0..SEGMENT_SLOTS {
            let id = format!("seg-{slot}-{segment}");
            match pieces.get(segment) {
                Some((text, hit)) => {
                    doc.visible(&id, true)?;
                    doc.text_ref(&id, text)?;
                    let color = if *hit {
                        self.palette.accent
                    } else {
                        self.palette.text
                    };
                    doc.foreground(&id, UiColor::from_rgb(color, 255))?;
                }
                None => doc.visible(&id, false)?,
            }
        }
        Ok(())
    }

    pub(super) fn paint_status(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let text = if self.walking {
            format!("Indexing · {} files", format::count(self.view().len()))
        } else if self.view().truncated {
            format!(
                "{} files · index limit reached",
                format::count(self.view().len())
            )
        } else {
            format!("{} files", format::count(self.view().len()))
        };
        doc.text("status", text)
    }

    pub(super) fn paint_overlays(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        doc.visible("preview", self.overlay == Overlay::Preview)?;
        doc.visible("details", self.overlay == Overlay::Details)?;
        doc.visible("help", self.overlay == Overlay::Help)?;
        doc.visible("duplicates", self.overlay == Overlay::Duplicates)?;
        doc.visible("setup", self.overlay == Overlay::Setup)?;
        doc.visible("verbs", self.sheet_open())?;
        match self.overlay {
            Overlay::None | Overlay::Help => Ok(()),
            Overlay::Verbs | Overlay::Confirm => self.paint_verbs(doc),
            Overlay::Preview => self.paint_preview(doc),
            Overlay::Details => self.paint_details(doc),
            Overlay::Duplicates => self.paint_duplicates(doc),
            Overlay::Setup => self.paint_setup(doc),
        }
    }

    pub(super) fn paint_preview(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let Some(entry) = self.selected_entry() else {
            return Ok(());
        };
        doc.text_ref("preview-name", entry.name())?;
        doc.text("preview-meta", meta_line(&entry, now_secs()))?;
        let lines = match &self.preview {
            Some(Preview::Text(lines)) => lines.as_slice(),
            _ => &[],
        };
        doc.visible("preview-empty", lines.is_empty())?;
        for slot in 0..PREVIEW_SLOTS {
            doc.text_ref(
                &format!("preview-line-{slot}"),
                lines.get(slot).map_or("", String::as_str),
            )?;
        }
        Ok(())
    }

    pub(super) fn paint_details(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let Some(entry) = self.selected_entry() else {
            return Ok(());
        };
        doc.text("details-name", entry.name().to_owned())?;
        doc.text("details-path", entry.path.clone())?;
        doc.text(
            "details-type",
            if entry.is_dir {
                "Folder".to_owned()
            } else {
                mark_for(&entry)
            },
        )?;
        doc.text(
            "details-size",
            if entry.is_dir {
                "-".to_owned()
            } else {
                format::bytes(entry.size)
            },
        )?;
        doc.text(
            "details-modified",
            format::relative_time(entry.modified, now_secs()),
        )
    }

    pub(super) fn paint_verbs(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let title = match (self.overlay, self.pending) {
            (Overlay::Confirm, Some(action)) => format!("{}?", action.label),
            _ => self
                .selected_entry()
                .map_or_else(|| "Actions".to_owned(), |entry| entry.name().to_owned()),
        };
        doc.text_ref("verbs-title", &title)?;
        for slot in 0..verbs::SLOTS {
            let verb = self.verbs.get(slot);
            let row = format!("verb-{slot}");
            doc.visible(&row, verb.is_some())?;
            doc.text_ref(
                &format!("verb-name-{slot}"),
                verb.map_or("", |verb| verbs::label(*verb)),
            )?;
            let alpha = if slot == self.verb { 36 } else { 0 };
            doc.background(&row, UiColor::from_rgb(0x00ff_ffff, alpha))?;
        }
        Ok(())
    }

    pub(super) fn paint_duplicates(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let summary = match self.groups.len() {
            0 => String::new(),
            1 => "1 group".to_owned(),
            n => format!("{n} groups"),
        };
        doc.text("dups-sub", summary)?;
        doc.visible("dups-empty", self.groups.is_empty())?;
        for slot in 0..DUPLICATE_SLOTS {
            let row = format!("dup-{slot}");
            let Some(group) = self.groups.get(slot) else {
                doc.visible(&row, false)?;
                continue;
            };
            doc.visible(&row, true)?;
            let first = self.view().entries[group[0]].clone();
            doc.text_ref(&format!("dup-name-{slot}"), first.name())?;
            doc.text(
                &format!("dup-meta-{slot}"),
                format!("{} copies · {}", group.len(), format::bytes(first.size)),
            )?;
            let places: Vec<String> = group
                .iter()
                .take(3)
                .map(|position| {
                    format::parent_for_display(&self.view().entries[*position].path, &self.home)
                })
                .collect();
            doc.text(&format!("dup-path-{slot}"), places.join("  ·  "))?;
        }
        Ok(())
    }

    pub(super) fn paint_setup(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let roots: Vec<String> = index::default_roots(env::var_os("HOME"))
            .iter()
            .map(|root| format::parent_for_display(&root.to_string_lossy(), &self.home))
            .collect();
        doc.text("setup-0", roots.join(", "))?;
        let truncated = if self.view().truncated {
            ", limit reached"
        } else {
            ""
        };
        doc.text(
            "setup-1",
            format!("{} files{truncated}", format::count(self.view().len())),
        )?;
        doc.text("setup-2", format!("{} folder levels", index::MAX_DEPTH))?;
        doc.text(
            "setup-3",
            format!("{} files at most", format::count(index::MAX_ENTRIES)),
        )?;
        let cache = index::cache_path(env::var_os("XDG_CACHE_HOME"), env::var_os("HOME"))
            .map_or_else(
                || "not available".to_owned(),
                |path| path.display().to_string(),
            );
        doc.text("setup-4", cache)
    }
}
