use flamewm_api::applications::DesktopApplication;
use flamewm_api::{OutputId, PanelEdge, Rect};
use flamewm_ui_core::style::ShellMetrics;
use flamewm_ui_core::{PopoverDirection, anchor_popover};

use crate::popup::work_area_for_panel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartCategory {
    All,
    /// Persisted user picks (`startFavorites`), matched by desktop application id.
    Favorites,
    Accessories,
    Development,
    Education,
    Games,
    Graphics,
    Internet,
    Multimedia,
    Office,
    Science,
    Settings,
    System,
    Other,
}

/// Single presentation-bucket table. Authority for the shell projection seam
/// (`application_matches_category` in `flamewm-shell`): raw desktop category
/// strings map to exactly one bucket; unlisted values fall to `Utilities`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartBucket {
    Development,
    Games,
    Graphics,
    Internet,
    Multimedia,
    System,
    Utilities,
}

impl StartCategory {
    #[must_use]
    pub fn from_application(application: &DesktopApplication) -> Self {
        Self::from_categories(&application.categories)
    }

    #[must_use]
    pub fn from_categories(categories: &[String]) -> Self {
        categories
            .iter()
            .map(|category| category.to_lowercase())
            .find_map(|category| match category.as_str() {
                "accessories" | "utility" => Some(Self::Accessories),
                "development" => Some(Self::Development),
                "education" => Some(Self::Education),
                "game" | "games" => Some(Self::Games),
                "graphics" => Some(Self::Graphics),
                "network" | "internet" => Some(Self::Internet),
                "audiovideo" | "audio" | "video" | "multimedia" => Some(Self::Multimedia),
                "office" => Some(Self::Office),
                "science" => Some(Self::Science),
                "settings" => Some(Self::Settings),
                "system" => Some(Self::System),
                _ => None,
            })
            .unwrap_or(Self::Other)
    }

    #[must_use]
    /// `favorite` is whether the application is in the persisted favorites list.
    fn accepts(self, category: StartCategory, favorite: bool) -> bool {
        match self {
            StartCategory::All => true,
            StartCategory::Favorites => favorite,
            _ => self == category,
        }
    }
}

/// Single category table: raw desktop category value -> presentation bucket.
/// Matches the shell projection mapping exactly, including the
/// `settings | system -> System` row and the `Utilities` default.
#[must_use]
pub fn presentation_key(raw_category: &str) -> Option<StartBucket> {
    match raw_category.to_ascii_lowercase().as_str() {
        "accessories" | "utility" | "education" | "office" | "science" => {
            Some(StartBucket::Utilities)
        }
        "development" => Some(StartBucket::Development),
        "game" | "games" => Some(StartBucket::Games),
        "graphics" => Some(StartBucket::Graphics),
        "network" | "internet" => Some(StartBucket::Internet),
        "audiovideo" | "audio" | "video" | "multimedia" => Some(StartBucket::Multimedia),
        "settings" | "system" => Some(StartBucket::System),
        _ => None,
    }
}

#[must_use]
pub fn bucket_for_application(application: &DesktopApplication) -> StartBucket {
    application
        .categories
        .iter()
        .find_map(|value| presentation_key(value))
        .unwrap_or(StartBucket::Utilities)
}

#[must_use]
pub fn start_category(application: &DesktopApplication) -> StartCategory {
    StartCategory::from_application(application)
}

/// Separator for the persisted `startFavorites` value: desktop ids never contain `;`.
const FAVORITES_SEPARATOR: char = ';';

/// Encodes the Favorites list for `startFavorites`: order kept, duplicates and ids that cannot
/// round-trip (empty, containing `;` or a newline) dropped.
#[must_use]
pub fn encode_favorites(ids: &[String]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for id in ids {
        let id = id.trim();
        if id.is_empty() || id.contains([FAVORITES_SEPARATOR, '\n', '\r']) || kept.contains(&id) {
            continue;
        }
        kept.push(id);
    }
    kept.join(&FAVORITES_SEPARATOR.to_string())
}

/// Decodes `startFavorites`: the inverse of [`encode_favorites`], tolerant of blanks and repeats.
#[must_use]
pub fn decode_favorites(text: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for id in text.split(FAVORITES_SEPARATOR).map(str::trim) {
        if !id.is_empty() && !ids.iter().any(|known| known == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}

#[must_use]
pub fn search_start_applications<'a>(
    applications: &'a [DesktopApplication],
    query: &str,
) -> Vec<&'a DesktopApplication> {
    StartModel::filter_slice(applications, StartCategory::All, query)
}

/// Precomputed per-application search index. Normalized once per set
/// replacement; query views borrow without cloning, lowercasing, or sorting.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StartEntry {
    name: String,
    id: String,
    generic_name: String,
    comment: String,
    keywords: Vec<String>,
    categories: Vec<String>,
    category: StartCategory,
}

impl StartEntry {
    fn build(application: &DesktopApplication) -> Self {
        Self {
            name: application.name.to_lowercase(),
            id: application.id.as_str().to_lowercase(),
            generic_name: application.generic_name.to_lowercase(),
            comment: application.comment.to_lowercase(),
            keywords: application
                .keywords
                .iter()
                .map(|keyword| keyword.to_lowercase())
                .collect(),
            categories: application
                .categories
                .iter()
                .map(|value| value.to_lowercase())
                .collect(),
            category: StartCategory::from_application(application),
        }
    }

    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || self.name.contains(needle)
            || self.id.contains(needle)
            || self.generic_name.contains(needle)
            || self.comment.contains(needle)
            || self.keywords.iter().any(|keyword| keyword.contains(needle))
            || self.categories.iter().any(|value| value.contains(needle))
    }
}

/// Indices into `entries`, sorted by (name, id) with stable ties.
fn sorted_indices(entries: &[StartEntry]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&left, &right| {
        entries[left]
            .name
            .cmp(&entries[right].name)
            .then_with(|| entries[left].id.cmp(&entries[right].id))
            .then_with(|| left.cmp(&right))
    });
    order
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartModel {
    applications: Vec<DesktopApplication>,
    entries: Vec<StartEntry>,
    /// Indices into `applications`, sorted by (name, id) with stable ties.
    sorted: Vec<usize>,
    /// Desktop application ids in the Favorites category, in the persisted order.
    favorites: Vec<String>,
    query: String,
    /// Bumped on every application-set replacement so callers can key caches on it.
    generation: u64,
}

impl StartModel {
    #[must_use]
    pub fn new(applications: Vec<DesktopApplication>) -> Self {
        let mut model = Self {
            applications,
            entries: Vec::new(),
            sorted: Vec::new(),
            favorites: Vec::new(),
            query: String::new(),
            generation: 0,
        };
        model.rebuild();
        model
    }

    pub fn replace_applications(&mut self, applications: Vec<DesktopApplication>) {
        self.applications = applications;
        self.rebuild();
    }

    #[must_use]
    pub fn applications(&self) -> &[DesktopApplication] {
        &self.applications
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
    }

    /// Replaces the Favorites list by application id and bumps the generation so caches refresh.
    /// Ids that name no known application and repeated ids are dropped; order is kept.
    pub fn set_favorites(&mut self, ids: Vec<String>) {
        let mut kept: Vec<String> = Vec::new();
        for id in ids {
            let known = self
                .applications
                .iter()
                .any(|application| application.id.as_str() == id);
            if known && !kept.contains(&id) {
                kept.push(id);
            }
        }
        self.favorites = kept;
        self.generation = self.generation.wrapping_add(1);
    }

    #[must_use]
    pub fn favorites(&self) -> &[String] {
        &self.favorites
    }

    /// Whether application `index` is listed in `category`, using the stored favorites.
    fn admits(&self, category: StartCategory, index: usize) -> bool {
        let id = self.applications[index].id.as_str();
        let favorite = self.favorites.iter().any(|favorite| favorite == id);
        category.accepts(self.entries[index].category, favorite)
    }

    /// Borrowed generation view: all applications in alpha order, no clone.
    #[must_use]
    pub fn results_view(&self) -> Vec<&DesktopApplication> {
        self.sorted
            .iter()
            .map(|&index| &self.applications[index])
            .collect()
    }

    /// Borrowed generation view: one category in alpha order, no clone.
    #[must_use]
    pub fn category_view(&self, category: StartCategory) -> Vec<&DesktopApplication> {
        self.sorted
            .iter()
            .filter(|&&index| self.admits(category, index))
            .map(|&index| &self.applications[index])
            .collect()
    }

    /// Borrowed generation view: ad-hoc query against the stored set.
    #[must_use]
    pub fn query_view(&self, query: &str) -> Vec<&DesktopApplication> {
        self.filter_view(StartCategory::All, query)
    }

    /// Borrowed generation view: category plus ad-hoc query.
    #[must_use]
    pub fn filter_view(&self, category: StartCategory, query: &str) -> Vec<&DesktopApplication> {
        let needle = query.trim().to_lowercase();
        self.sorted
            .iter()
            .filter(|&&index| self.admits(category, index))
            .filter(|&&index| self.entries[index].matches(&needle))
            .map(|&index| &self.applications[index])
            .collect()
    }

    /// Borrowed generation view: category plus ad-hoc query, windowed.
    #[must_use]
    pub fn slot_view(
        &self,
        category: StartCategory,
        query: &str,
        first: usize,
        count: usize,
    ) -> Vec<&DesktopApplication> {
        self.filter_view(category, query)
            .into_iter()
            .skip(first)
            .take(count)
            .collect()
    }

    #[must_use]
    pub fn results(&self) -> Vec<&DesktopApplication> {
        self.query_view(&self.query)
    }

    #[must_use]
    pub fn results_for_category(&self, category: StartCategory) -> Vec<&DesktopApplication> {
        self.filter_view(category, &self.query)
    }

    #[must_use]
    pub fn results_in_slots(
        &self,
        first_slot: usize,
        slot_count: usize,
    ) -> Vec<&DesktopApplication> {
        self.slot_view(StartCategory::All, &self.query, first_slot, slot_count)
    }

    fn rebuild(&mut self) {
        self.entries = self.applications.iter().map(StartEntry::build).collect();
        self.sorted = sorted_indices(&self.entries);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Changes whenever the application set is replaced; equal values mean equal indices.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// All application indices in (name, id) order, as `results_view` lists them.
    #[must_use]
    pub fn sorted_order(&self) -> &[usize] {
        &self.sorted
    }

    /// Indices into `applications()` for a category plus ad-hoc query, in alpha order.
    #[must_use]
    pub fn filter_indices(&self, category: StartCategory, query: &str) -> Vec<usize> {
        let needle = query.trim().to_lowercase();
        self.sorted
            .iter()
            .copied()
            .filter(|&index| self.admits(category, index))
            .filter(|&index| self.entries[index].matches(&needle))
            .collect()
    }

    /// Slice path used by the free-function shims. Normalizes each entry once
    /// and sorts indices instead of cloning or sorting the applications.
    fn filter_slice<'a>(
        applications: &'a [DesktopApplication],
        category: StartCategory,
        query: &str,
    ) -> Vec<&'a DesktopApplication> {
        let needle = query.trim().to_lowercase();
        let entries: Vec<StartEntry> = applications.iter().map(StartEntry::build).collect();
        sorted_indices(&entries)
            .into_iter()
            .filter(|&index| category.accepts(entries[index].category, false))
            .filter(|&index| entries[index].matches(&needle))
            .map(|index| &applications[index])
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartSurfaceLayout {
    pub button: flamewm_api::Rect,
    pub anchor: flamewm_api::Rect,
    pub popover: flamewm_api::Rect,
}

#[must_use]
/// Start button anchor on the live taskbar `panel_rect`; the popover is clamped to the output's
/// work area above/beside that taskbar, so it never extends under the bar.
pub fn start_surface_layout(
    output: OutputId,
    output_rect: Rect,
    panel_rect: Rect,
    edge: PanelEdge,
    metrics: ShellMetrics,
) -> StartSurfaceLayout {
    let (button_w, button_h) = if edge.is_horizontal() {
        (
            i32::from(metrics.start_button_width),
            i32::from(metrics.panel_height),
        )
    } else {
        (
            i32::from(metrics.panel_height),
            i32::from(metrics.start_button_width),
        )
    };
    let anchor = match edge {
        PanelEdge::Right => Rect::new(
            panel_rect.right() - button_w,
            panel_rect.y,
            button_w,
            button_h,
        ),
        PanelEdge::Bottom | PanelEdge::Top | PanelEdge::Left => {
            Rect::new(panel_rect.x, panel_rect.y, button_w, button_h)
        }
    };
    let work_area = work_area_for_panel(output_rect, panel_rect, edge);
    let direction = match edge {
        PanelEdge::Bottom => PopoverDirection::Above,
        PanelEdge::Top => PopoverDirection::Below,
        PanelEdge::Left => PopoverDirection::RightOf,
        PanelEdge::Right => PopoverDirection::LeftOf,
    };
    let popover = anchor_popover(
        output,
        work_area,
        anchor,
        (
            i32::from(metrics.start_menu_width),
            i32::from(metrics.start_menu_min_height),
        ),
        direction,
        i32::from(metrics.popover_offset),
    )
    .rect;
    StartSurfaceLayout {
        button: Rect::new(0, 0, button_w, button_h),
        anchor,
        popover,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::DesktopAppId;

    fn application(name: &str) -> DesktopApplication {
        DesktopApplication {
            id: DesktopAppId::new("org.example.editor"),
            name: name.to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: String::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn search_matches_generic_name_and_comment() {
        let mut app = application("Editor");
        app.generic_name = "Document Writer".to_owned();
        app.comment = "Create and edit text files".to_owned();
        let applications = vec![app];

        assert_eq!(search_start_applications(&applications, "writer").len(), 1);
        assert_eq!(
            search_start_applications(&applications, "TEXT FILES").len(),
            1
        );
    }

    #[test]
    fn empty_search_returns_all_sorted_applications() {
        let applications = vec![application("Zulu"), application("Alpha")];
        let results = search_start_applications(&applications, "  ");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "Alpha");
        assert_eq!(results[1].name, "Zulu");
    }

    #[test]
    fn model_views_match_free_functions_without_requery_work() {
        let mut game = application("Zulu Game");
        game.categories = vec!["Game".to_owned()];
        game.keywords = vec!["Arcade".to_owned()];
        let mut dev = application("Alpha Editor");
        dev.generic_name = "Code Writer".to_owned();
        dev.categories = vec!["Development".to_owned()];
        let applications = vec![game, dev];
        let mut model = StartModel::new(applications.clone());

        assert_eq!(
            model
                .results_view()
                .iter()
                .map(|app| app.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha Editor", "Zulu Game"]
        );
        assert_eq!(model.category_view(StartCategory::Games).len(), 1);
        assert_eq!(model.query_view("WRITER").len(), 1);
        assert_eq!(
            model.query_view("arcade").len(),
            search_start_applications(&applications, "arcade").len()
        );
        model.set_query("alpha");
        assert_eq!(model.results().len(), 1);
        assert_eq!(
            model.results_for_category(StartCategory::Development).len(),
            1
        );
        assert_eq!(model.results_for_category(StartCategory::Games).len(), 0);
        assert_eq!(model.results_in_slots(0, 1).len(), 1);
        model.replace_applications(vec![application("Solo")]);
        assert_eq!(model.results_view().len(), 1);
    }

    #[test]
    fn presentation_table_matches_shell_projection_seam() {
        assert_eq!(presentation_key("Utility"), Some(StartBucket::Utilities));
        assert_eq!(presentation_key("Settings"), Some(StartBucket::System));
        assert_eq!(presentation_key("Office"), Some(StartBucket::Utilities));
        assert_eq!(presentation_key("Unknown"), None);
        let mut app = application("Helper");
        app.categories = vec!["Settings".to_owned()];
        assert_eq!(bucket_for_application(&app), StartBucket::System);
        let fallback = application("Plain");
        assert_eq!(bucket_for_application(&fallback), StartBucket::Utilities);
    }

    fn named(id: &str, name: &str, categories: &[&str]) -> DesktopApplication {
        let mut app = application(name);
        app.id = DesktopAppId::new(id);
        app.categories = categories.iter().map(|c| (*c).to_owned()).collect();
        app
    }

    #[test]
    fn favorites_accepts_only_listed_applications_in_any_category() {
        assert!(StartCategory::Favorites.accepts(StartCategory::Games, true));
        assert!(!StartCategory::Favorites.accepts(StartCategory::Games, false));
        assert!(StartCategory::All.accepts(StartCategory::Games, false));
        assert!(!StartCategory::Games.accepts(StartCategory::Office, true));
    }

    #[test]
    fn favorites_view_lists_only_starred_apps_sorted_by_name() {
        let mut model = StartModel::new(vec![
            named("b.desktop", "Calc", &["Utility"]),
            named("a.desktop", "beta", &["Game"]),
            named("c.desktop", "Alpha", &["Graphics"]),
        ]);
        model.set_favorites(vec!["a.desktop".into(), "c.desktop".into()]);
        let names: Vec<&str> = model
            .category_view(StartCategory::Favorites)
            .iter()
            .map(|app| app.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "beta"]);
    }

    #[test]
    fn all_applications_sort_case_insensitively_by_name() {
        let model = StartModel::new(vec![
            named("z.desktop", "zeta", &[]),
            named("m.desktop", "Mail", &[]),
            named("a.desktop", "alpha", &[]),
        ]);
        let names: Vec<&str> = model
            .results_view()
            .iter()
            .map(|app| app.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "Mail", "zeta"]);
    }

    #[test]
    fn search_respects_favorites_and_category_filters() {
        let mut model = StartModel::new(vec![
            named("calc.desktop", "Calculator", &["Utility"]),
            named("gimp.desktop", "GIMP", &["Graphics"]),
        ]);
        model.set_favorites(vec!["gimp.desktop".into()]);
        assert_eq!(model.filter_view(StartCategory::Favorites, "calc").len(), 0);
        assert_eq!(model.filter_view(StartCategory::Favorites, "gim").len(), 1);
        assert_eq!(model.filter_view(StartCategory::All, "calc").len(), 1);
        assert_eq!(model.filter_view(StartCategory::Graphics, "gim").len(), 1);
    }

    #[test]
    fn set_favorites_drops_unknown_and_repeated_ids_and_keeps_order() {
        let mut model = StartModel::new(vec![
            named("calc.desktop", "Calculator", &[]),
            named("gimp.desktop", "GIMP", &[]),
        ]);
        model.set_favorites(vec![
            "ghost.desktop".into(),
            "gimp.desktop".into(),
            "calc.desktop".into(),
            "gimp.desktop".into(),
        ]);
        assert_eq!(model.favorites(), ["gimp.desktop", "calc.desktop"]);
        model.set_favorites(vec!["ghost.desktop".into()]);
        assert!(model.favorites().is_empty());
        assert!(model.category_view(StartCategory::Favorites).is_empty());
    }

    #[test]
    fn favorites_round_trip_through_the_setting_text() {
        let ids: Vec<String> = vec!["gimp.desktop".into(), "org.calc.desktop".into()];
        let text = encode_favorites(&ids);
        assert_eq!(text, "gimp.desktop;org.calc.desktop");
        assert_eq!(decode_favorites(&text), ids);
        assert_eq!(decode_favorites(" a.desktop ;; a.desktop;"), ["a.desktop"]);
        assert_eq!(
            encode_favorites(&["x;y".into(), "".into(), "ok".into()]),
            "ok"
        );
        assert_eq!(decode_favorites(""), Vec::<String>::new());
    }
}
