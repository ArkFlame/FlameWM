use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WidgetStore {
    pub(super) widgets: Vec<Widget>,
    /// Grid (cols, rows) the cells were last laid out on; drives proportional reflow.
    pub(super) grid_size: Option<(i32, i32)>,
}

impl WidgetStore {
    #[must_use]
    pub fn widgets(&self) -> &[Widget] {
        &self.widgets
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.widgets.len() >= MAX_WIDGETS
    }

    #[must_use]
    pub fn get(&self, id: u32) -> Option<&Widget> {
        self.widgets.iter().find(|widget| widget.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Widget> {
        self.widgets.iter_mut().find(|widget| widget.id == id)
    }

    pub fn add(&mut self, kind: WidgetKind, bounds: Rect) -> Option<u32> {
        self.add_on_grid(kind, GridMetrics::for_work_area(bounds, 100))
    }

    /// Adds a widget in the free cell nearest the centre of the grid, at the kind's
    /// default size. Returns `None` when the store is full or nothing fits.
    pub fn add_on_grid(&mut self, kind: WidgetKind, metrics: GridMetrics) -> Option<u32> {
        if self.is_full() {
            return None;
        }
        self.layout_grid(metrics);
        let span = widget_grid::default_span(kind);
        let placement = metrics.nearest_free_to(span, metrics.center(), &self.placements())?;
        let id = self
            .widgets
            .iter()
            .map(|widget| widget.id)
            .max()
            .map_or(1, |id| id.saturating_add(1));
        let mut widget = Widget::new(id, kind, Point::new(0, 0));
        widget.grid = Some(placement);
        widget.rect = metrics.rect_of(placement);
        widget.anchor = Some(EdgeAnchor::of(widget.rect, metrics.work));
        self.widgets.push(widget);
        Some(id)
    }

    /// Grid placements of every grid-placed widget.
    #[must_use]
    pub fn placements(&self) -> Vec<Placement> {
        self.widgets
            .iter()
            .filter_map(|widget| widget.grid)
            .collect()
    }

    /// Moves a grid widget to `placement` if its kind allows that size, it fits, and it
    /// overlaps no other widget. Locked widgets never move.
    pub fn place_on_grid(&mut self, id: u32, placement: Placement, metrics: GridMetrics) -> bool {
        let placement = metrics.from_right(placement);
        let Some(widget) = self.get(id) else {
            return false;
        };
        if widget.locked
            || !widget_grid::allowed_spans(widget.kind).contains(&placement.span)
            || !placement.fits(metrics)
            || self
                .placements_except(id)
                .iter()
                .any(|other| metrics.collides(placement, *other))
        {
            return false;
        }
        self.set_cell(id, placement, metrics);
        true
    }

    pub(super) fn set_cell(&mut self, id: u32, placement: Placement, metrics: GridMetrics) {
        if let Some(widget) = self.get_mut(id) {
            widget.grid = Some(placement);
            widget.rect = metrics.rect_of(placement);
            widget.anchor = Some(EdgeAnchor::of(widget.rect, metrics.work));
        }
    }

    /// Another unlocked widget of the same size whose cell is exactly `want`, when it is
    /// the only widget there. Dropping onto it swaps the two.
    pub(super) fn swap_partner(
        &self,
        id: u32,
        want: Placement,
        metrics: GridMetrics,
    ) -> Option<u32> {
        let span = self.get(id)?.grid?.span;
        let others: Vec<(u32, Placement)> = self
            .widgets
            .iter()
            .filter(|widget| widget.id != id)
            .filter_map(|widget| widget.grid.map(|cell| (widget.id, cell)))
            .collect();
        let hits: Vec<&(u32, Placement)> = others
            .iter()
            .filter(|(_, cell)| metrics.collides(want, *cell))
            .collect();
        match hits.as_slice() {
            [(partner, cell)]
                if cell.span == span
                    && cell.col == want.col
                    && cell.row == want.row
                    && self.get(*partner).is_some_and(|widget| !widget.locked) =>
            {
                Some(*partner)
            }
            _ => None,
        }
    }

    /// Drops a grid widget whose top-left is `origin`. Snaps to the nearest cell; a
    /// same-size widget there is swapped with the dragged one, otherwise the dragged
    /// widget takes the nearest free cell (push).
    pub fn drop_on_grid(&mut self, id: u32, origin: Point, metrics: GridMetrics) -> bool {
        let Some(span) = self
            .get(id)
            .and_then(|widget| widget.grid)
            .map(|cell| cell.span)
        else {
            return false;
        };
        let want = metrics.snap(origin, span);
        if let Some(partner) = self.swap_partner(id, want, metrics) {
            let Some(old) = self.get(id).and_then(|widget| widget.grid) else {
                return false;
            };
            self.set_cell(partner, old, metrics);
            self.set_cell(id, want, metrics);
            return true;
        }
        match self.preview_drop(id, origin, metrics) {
            Some(slot) => self.place_on_grid(id, slot, metrics),
            None => false,
        }
    }

    /// Cell a drop at `origin` would take (the drag ghost): the snapped cell when it is
    /// free or holds a swap partner, otherwise the nearest free cell (collision push).
    /// `None` when nothing fits.
    #[must_use]
    pub fn preview_drop(&self, id: u32, origin: Point, metrics: GridMetrics) -> Option<Placement> {
        let span = self.get(id).and_then(|widget| widget.grid)?.span;
        let want = metrics.snap(origin, span);
        if self.swap_partner(id, want, metrics).is_some() {
            return Some(want);
        }
        metrics.nearest_free(want, &self.placements_except(id))
    }

    /// Cell an edge drag to `want` would take: `want` when free, else the nearest free cell.
    #[must_use]
    pub fn preview_placement(
        &self,
        id: u32,
        want: Placement,
        metrics: GridMetrics,
    ) -> Option<Placement> {
        let want = metrics.from_right(want);
        metrics.nearest_free(want, &self.placements_except(id))
    }

    /// Places widget `id` at `want`, or at the nearest free cell when `want` is taken.
    pub fn place_nearest(&mut self, id: u32, want: Placement, metrics: GridMetrics) -> bool {
        match self.preview_placement(id, want, metrics) {
            Some(slot) => self.place_on_grid(id, slot, metrics),
            None => false,
        }
    }

    pub(super) fn placements_except(&self, id: u32) -> Vec<Placement> {
        self.widgets
            .iter()
            .filter(|widget| widget.id != id)
            .filter_map(|widget| widget.grid)
            .collect()
    }

    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.widgets.len();
        self.widgets.retain(|widget| widget.id != id);
        before != self.widgets.len()
    }

    pub fn set_rect(&mut self, id: u32, rect: Rect) -> bool {
        if !rect.is_valid() {
            return false;
        }
        let Some(widget) = self.get_mut(id) else {
            return false;
        };
        if widget.locked || widget.rect == rect {
            return false;
        }
        widget.rect = rect;
        true
    }

    pub fn toggle_lock(&mut self, id: u32) -> Option<bool> {
        let widget = self.get_mut(id)?;
        widget.locked = !widget.locked;
        Some(widget.locked)
    }

    /// Re-records the horizontal anchor of a widget after a user move or resize.
    pub fn anchor_to(&mut self, id: u32, bounds: Rect) {
        if let Some(widget) = self.get_mut(id) {
            widget.anchor = Some(EdgeAnchor::of(widget.rect, bounds));
        }
    }

    /// Places every widget from its edge anchor for `bounds`, so a right-side
    /// widget keeps its distance from the right edge when the screen changes.
    /// Unresolved (legacy absolute) anchors pick the nearest side here.
    pub fn clamp_all(&mut self, bounds: Rect) {
        self.layout_grid(GridMetrics::for_work_area(bounds, 100));
    }

    /// Lays every widget out on `metrics`. Legacy right-anchored cells become left columns,
    /// sizes are clamped to what each kind allows, and cells reflow proportionally from the
    /// previous grid size. Legacy free-position widgets snap to the nearest free cell; a
    /// legacy widget with no free cell keeps its clamped pixel rect (non-destructive).
    pub fn layout_grid(&mut self, metrics: GridMetrics) {
        let bounds = metrics.work;
        for widget in &mut self.widgets {
            if let Some(cell) = widget.grid {
                let cell = metrics.from_right(cell);
                widget.grid = Some(Placement {
                    span: widget_grid::fit_span(widget.kind, cell.span),
                    ..cell
                });
            }
        }
        let placed_items: Vec<Placement> = self.placements();
        let mut slots = metrics.reflow(&placed_items, self.grid_size).into_iter();
        for widget in &mut self.widgets {
            if widget.grid.is_some() {
                widget.grid = slots.next().flatten();
            }
        }
        let mut placed = self.placements();
        for widget in &mut self.widgets {
            if widget.grid.is_none() {
                let span = widget_grid::default_span(widget.kind);
                if let Some(slot) = metrics.migrate(widget.rect, span, &placed) {
                    placed.push(slot);
                    widget.grid = Some(slot);
                }
            }
        }
        for widget in &mut self.widgets {
            widget.rect = if let Some(placement) = widget.grid {
                metrics.rect_of(placement)
            } else {
                let x = widget.anchor.map_or(widget.rect.x, |anchor| {
                    anchor.x_in(widget.rect.width, bounds)
                });
                Rect::new(x, widget.rect.y, widget.rect.width, widget.rect.height)
                    .clamp_inside(bounds)
            };
            widget.anchor = Some(EdgeAnchor::of(widget.rect, bounds));
        }
        self.grid_size = Some((metrics.cols, metrics.rows));
    }
}

#[must_use]
pub fn move_widget_rect(original: Rect, dx: i32, dy: i32, bounds: Rect) -> Rect {
    Rect::new(
        original.x.saturating_add(dx),
        original.y.saturating_add(dy),
        original.width,
        original.height,
    )
    .clamp_inside(bounds)
}

pub fn state_path() -> FlameResult<PathBuf> {
    let base = flamewm_api::paths::xdg_dir("XDG_STATE_HOME", ".local/state")
        .ok_or_else(|| FlameError::invalid("HOME is not set"))?;
    Ok(base.join(STATE_FILE_NAME))
}

pub fn load(path: &Path) -> FlameResult<WidgetStore> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(WidgetStore::default()),
        Err(error) => Err(io_error("read desktop widgets", error)),
    }
}

pub fn persist(path: &Path, store: &WidgetStore) -> FlameResult<()> {
    flamewm_desktop_core::atomic_file::write_text(path, "desktop widgets", &serialize(store))
}

#[must_use]
pub fn serialize(store: &WidgetStore) -> String {
    let mut output = String::from("v1\n");
    if let Some((cols, rows)) = store.grid_size {
        output.push_str(&format!("grid\t{cols}\t{rows}\n"));
    }
    for widget in &store.widgets {
        let values = widget
            .values
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(",");
        let anchor = widget.anchor.map_or_else(String::new, |anchor| {
            format!("{}{}", if anchor.right { 'r' } else { 'l' }, anchor.gap)
        });
        let grid = widget.grid.map_or_else(String::new, |cell| {
            format!(
                "{},{},{},{},{}",
                cell.col,
                cell.row,
                cell.span.w,
                cell.span.h,
                if cell.right { 'r' } else { 'l' }
            )
        });
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            widget.id,
            widget.kind.id(),
            widget.rect.x,
            widget.rect.y,
            widget.rect.width,
            widget.rect.height,
            i32::from(widget.locked),
            hex(widget.template.as_bytes()),
            anchor,
            values,
            grid
        ));
    }
    output
}

pub fn parse(text: &str) -> FlameResult<WidgetStore> {
    let mut lines = text.lines();
    if lines.next() != Some("v1") {
        return Err(FlameError::invalid("invalid desktop widget state header"));
    }
    let mut store = WidgetStore::default();
    // Optional grid line written after the header (absent in older files).
    let mut lines = lines.peekable();
    if let Some(line) = lines.peek().copied() {
        if let Some(size) = line.strip_prefix("grid\t") {
            let (cols, rows) = size
                .split_once('\t')
                .ok_or_else(|| FlameError::invalid("invalid desktop widget grid size"))?;
            store.grid_size = Some((number(cols)?, number(rows)?));
            lines.next();
        }
    }
    for line in lines.filter(|line| !line.is_empty()) {
        if store.is_full() {
            break;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        // 9 fields: legacy record without an edge anchor (resolved on first clamp).
        // 11 fields: current record with a trailing grid cell (absent in older records).
        let (anchor_field, values_field, grid_field) = match fields.len() {
            9 => ("", fields[8], ""),
            10 => (fields[8], fields[9], ""),
            11 => (fields[8], fields[9], fields[10]),
            _ => return Err(FlameError::invalid("invalid desktop widget record")),
        };
        let grid = parse_grid(grid_field)?;
        let anchor = match anchor_field.chars().next() {
            None => None,
            Some(side @ ('l' | 'r')) => Some(EdgeAnchor {
                right: side == 'r',
                gap: number(&anchor_field[1..])?,
            }),
            Some(_) => return Err(FlameError::invalid("invalid desktop widget anchor")),
        };
        let Some(kind) = WidgetKind::from_id(fields[1]) else {
            continue;
        };
        let id = number::<u32>(fields[0])?;
        if store.get(id).is_some() {
            return Err(FlameError::invalid("duplicate desktop widget id"));
        }
        let rect = Rect::new(
            number(fields[2])?,
            number(fields[3])?,
            number::<i32>(fields[4])?.max(MIN_WIDGET_SIZE),
            number::<i32>(fields[5])?.max(MIN_WIDGET_SIZE),
        );
        let locked = match fields[6] {
            "0" => false,
            "1" => true,
            _ => return Err(FlameError::invalid("invalid desktop widget lock flag")),
        };
        let template = String::from_utf8(unhex(
            fields[7],
            "odd desktop widget template",
            "invalid desktop widget template",
        )?)
        .map_err(|_| FlameError::invalid("desktop widget template is not UTF-8"))?;
        let mut widget = Widget::new(id, kind, rect.origin());
        widget.rect = rect;
        widget.locked = locked;
        widget.template = template;
        widget.anchor = anchor;
        widget.grid = grid;
        let mut pairs = Vec::new();
        for pair in values_field.split(',').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| FlameError::invalid("invalid desktop widget option"))?;
            pairs.push((key, number(value)?));
        }
        apply_option_pairs(&mut widget, &pairs);
        store.widgets.push(widget);
    }
    Ok(store)
}

/// Applies saved option pairs and migrates retired ones: `inheritSurface=0` or any saved
/// per-card surface override becomes `customSurface=1`, an explicit accent becomes
/// `customAccent=1`, and the removed `radius`, `backdropBlur` and `dateSize` are dropped.
pub(super) fn apply_option_pairs(widget: &mut Widget, pairs: &[(&str, i64)]) {
    let inherit = pairs
        .iter()
        .find(|(key, _)| *key == "inheritSurface")
        .map(|(_, value)| *value);
    let overrides = pairs
        .iter()
        .any(|(key, _)| SURFACE_OVERRIDE_KEYS.contains(key));
    for &(key, value) in pairs {
        match key {
            "inheritSurface" | "radius" | "backdropBlur" | "dateSize" => {}
            "accentColor" => {
                widget.set_value(key, value);
                if value != ACCENT_COLOR {
                    widget.set_value("customAccent", 1);
                }
            }
            _ => {
                widget.set_value(key, value);
            }
        }
    }
    if inherit == Some(0) || (inherit.is_none() && overrides) {
        widget.set_value("customSurface", 1);
    }
}

/// Parses `col,row,w,h,side` (side `l` or `r`); an empty field is no grid cell.
pub(super) fn parse_grid(field: &str) -> FlameResult<Option<Placement>> {
    if field.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = field.split(',').collect();
    if parts.len() != 5 {
        return Err(FlameError::invalid("invalid desktop widget grid cell"));
    }
    let right = match parts[4] {
        "l" => false,
        "r" => true,
        _ => return Err(FlameError::invalid("invalid desktop widget grid side")),
    };
    Ok(Some(Placement {
        col: number(parts[0])?,
        row: number(parts[1])?,
        span: Span {
            w: number(parts[2])?,
            h: number(parts[3])?,
        },
        right,
    }))
}

pub(super) fn number<T: std::str::FromStr>(value: &str) -> FlameResult<T> {
    value
        .parse()
        .map_err(|_| FlameError::invalid("invalid desktop widget number"))
}
