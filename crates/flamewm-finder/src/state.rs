//! Session, selection and verb/confirm state transitions for the finder window.

use super::*;

impl Finder {
    pub(super) fn new(
        home: String,
        index: Shared,
        theme: SettingsSnapshot,
        pid_file: PathBuf,
    ) -> Self {
        let palette = Palette::for_theme(&theme);
        let mut app = Self {
            home,
            index,
            walking: true,
            query: String::new(),
            parsed: Query::default(),
            hits: Hits::default(),
            selected: 0,
            first: 0,
            navigating: false,
            placeholder: 0,
            overlay: Overlay::None,
            preview: None,
            grid: false,
            groups: Vec::new(),
            theme,
            palette,
            pid_file,
            last_click: None,
            apps: Vec::new(),
            answer: None,
            app_hits: Vec::new(),
            actions: Vec::new(),
            logout: false,
            pending: None,
            verbs: &[],
            verb: 0,
            frecency: quick::Frecency::default(),
            frecency_file: None,
        };
        app.refresh();
        app
    }

    pub(super) fn view(&self) -> std::sync::RwLockReadGuard<'_, Index> {
        self.index.read()
    }

    pub(super) fn refresh(&mut self) {
        self.rerank();
        self.selected = 0;
        self.first = 0;
    }

    /// Recomputes the results for the current query, keeping the selection where it is.
    pub(super) fn rerank(&mut self) {
        self.parsed = Query::parse(&self.query, &self.home);
        let now = now_secs();
        let files = palette::mode(&self.query) == Mode::Files;
        self.hits = if files {
            search::rank(&self.view(), &self.parsed, now)
        } else {
            Hits::default()
        };
        let quick = files && !self.query.trim().is_empty();
        self.answer = quick.then(|| quick::calculate(&self.query)).flatten();
        self.app_hits = if quick {
            quick::rank_apps(&self.apps, &self.query, &self.frecency, now)
        } else {
            Vec::new()
        };
        self.actions = if quick {
            quick::actions_matching(&self.query, self.logout)
        } else {
            Vec::new()
        };
        let len = self.items().len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    /// Quick rows above the file results: calculator answer, applications, then system actions.
    pub(super) fn quick_count(&self) -> usize {
        usize::from(self.answer.is_some()) + self.app_hits.len() + self.actions.len()
    }

    pub(super) fn accept_session(&mut self, logout: bool) {
        self.logout = logout;
        self.rerank();
    }

    /// Cached files are shown but do not end the indexing state, since the fresh walk follows.
    pub(super) fn accept_cached(&mut self) {
        self.refresh();
    }

    pub(super) fn accept_apps(&mut self, apps: Vec<DesktopApplication>) {
        self.apps = apps;
        self.refresh();
    }

    pub(super) fn launch_app(&mut self, position: usize) {
        let Some(app) = self.apps.get(position) else {
            return;
        };
        if let Err(error) =
            flamewm_applications::launch_argv(&app.argv, &ApplicationLaunchOptions::default())
        {
            eprintln!("flamewm-finder: launch {} failed: {error}", app.id.as_str());
            return;
        }
        self.frecency.record(app.id.as_str(), now_secs());
        if let Some(path) = &self.frecency_file
            && let Err(error) = self.frecency.save(path)
        {
            eprintln!("flamewm-finder: frecency save failed: {error}");
        }
    }

    pub(super) fn run_quick_action(&self, action: &quick::SystemAction) {
        if let Err(error) = quick::run_action(action) {
            eprintln!("flamewm-finder: {} failed: {error}", action.label);
        }
    }

    pub(super) fn set_query(&mut self, query: String) {
        self.query = query;
        self.refresh();
    }

    /// Items of the current view: result files, commands, or folder scopes.
    pub(super) fn items(&self) -> Vec<Item> {
        match palette::mode(&self.query) {
            Mode::Files => {
                let mut items: Vec<Item> = self.answer.iter().cloned().map(Item::Calc).collect();
                items.extend(self.app_hits.iter().map(|&i| Item::App(i)));
                items.extend(self.actions.iter().copied().map(Item::Action));
                items.extend(self.hits.indices.iter().map(|&i| Item::Entry(i)));
                items
            }
            Mode::Commands(filter) => palette::command_matches(&filter)
                .into_iter()
                .map(Item::Command)
                .collect(),
            Mode::Scopes(filter) => palette::scope_choices(&self.home, &filter)
                .into_iter()
                .map(Item::Scope)
                .collect(),
        }
    }

    pub(super) fn selected_item(&self) -> Option<Item> {
        self.items().into_iter().nth(self.selected)
    }

    /// True when results appear as tiles: always for the home view, and for results once the user
    /// has switched to the grid.
    /// Quick rows are list rows, so a query with quick rows always shows the list.
    pub(super) fn tiles_shown(&self) -> bool {
        palette::mode(&self.query) == Mode::Files
            && !self.hits.indices.is_empty()
            && self.quick_count() == 0
            && (self.grid || self.query.is_empty())
    }

    pub(super) fn selected_entry(&self) -> Option<Entry> {
        match self.selected_item()? {
            Item::Entry(position) => self.view().entries.get(position).cloned(),
            _ => None,
        }
    }

    pub(super) fn select(&mut self, position: usize) {
        let len = self.items().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = position.min(len - 1);
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + ROW_SLOTS {
            self.first = self.selected + 1 - ROW_SLOTS;
        }
        if self.overlay == Overlay::Preview {
            self.preview = self
                .selected_entry()
                .map(|entry| preview::load(Path::new(&entry.path), entry.is_dir));
        }
    }

    pub(super) fn step(&mut self, delta: isize) {
        let target = self.selected.saturating_add_signed(delta);
        self.select(target);
    }

    /// Rows moved by Up and Down: one in the list, a grid row of tiles in tile view.
    pub(super) fn vertical_step(&self) -> isize {
        if self.tiles_shown() {
            isize::try_from(GRID_COLUMNS).unwrap_or(4)
        } else {
            1
        }
    }

    pub(super) fn accept_index(&mut self) {
        self.walking = false;
        self.refresh();
    }

    /// The live watcher changed the index: results update, the selection stays put, and an open
    /// duplicates list is recomputed because its positions may have moved.
    pub(super) fn accept_changes(&mut self) {
        self.rerank();
        if self.overlay == Overlay::Duplicates {
            let groups = index::duplicate_groups(&self.view(), DUPLICATE_SLOTS);
            self.groups = groups;
        }
    }

    /// Tab opens the actions for the selected file or app; Tab again closes the sheet.
    pub(super) fn toggle_verbs(&mut self) {
        if self.overlay == Overlay::Verbs {
            self.overlay = Overlay::None;
            return;
        }
        let subject = match self.selected_item() {
            Some(Item::Entry(_)) => verbs::Subject::File,
            Some(Item::App(_)) => verbs::Subject::App,
            _ => return,
        };
        self.verbs = verbs::for_subject(subject);
        self.verb = 0;
        self.overlay = Overlay::Verbs;
    }

    pub(super) fn move_verb(&mut self, delta: isize) {
        let last = self.verbs.len().saturating_sub(1);
        self.verb = self.verb.saturating_add_signed(delta).min(last);
    }

    /// Closes the sheet and runs the action in `slot`.
    pub(super) fn run_verb(&mut self, slot: usize) {
        let Some(verb) = self.verbs.get(slot).copied() else {
            return;
        };
        self.overlay = Overlay::None;
        match verb {
            verbs::Verb::Open | verbs::Verb::Launch => self.activate_selected(),
            verbs::Verb::Reveal => self.open_selected(true),
            verbs::Verb::Preview => self.toggle(Overlay::Preview),
            verbs::Verb::Details => self.toggle(Overlay::Details),
            verbs::Verb::Cancel => {}
            verbs::Verb::Confirm => {
                if let Some(action) = self.pending.take() {
                    self.run_quick_action(action);
                }
            }
        }
    }

    /// Tab's sheet or the confirm sheet is open; its rows take Up, Down and Enter.
    pub(super) fn sheet_open(&self) -> bool {
        matches!(self.overlay, Overlay::Verbs | Overlay::Confirm)
    }

    /// Asks before a session-ending action. The sheet starts on Cancel.
    pub(super) fn open_confirm(&mut self, action: &'static quick::SystemAction) {
        self.pending = Some(action);
        self.verbs = verbs::CONFIRM_VERBS;
        self.verb = 0;
        self.overlay = Overlay::Confirm;
    }

    pub(super) fn toggle(&mut self, overlay: Overlay) {
        if self.overlay == overlay {
            self.overlay = Overlay::None;
            return;
        }
        if overlay == Overlay::Preview {
            self.preview = self
                .selected_entry()
                .map(|entry| preview::load(Path::new(&entry.path), entry.is_dir));
        }
        self.overlay = overlay;
    }

    pub(super) fn open_path(&self, target: &Path) {
        if let Err(error) = Process::new("xdg-open").arg(target).spawn() {
            eprintln!("flamewm-finder: open failed: {}: {error}", target.display());
        }
    }

    pub(super) fn open_selected(&self, reveal: bool) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let target = if reveal {
            Path::new(&entry.path)
                .parent()
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
        } else {
            PathBuf::from(&entry.path)
        };
        self.open_path(&target);
    }

    /// Enter: open a file, run a command, or write a folder scope into the search box.
    pub(super) fn activate_selected(&mut self) {
        match self.selected_item() {
            Some(Item::Entry(_)) => self.open_selected(false),
            Some(Item::Command(spec)) => self.run_command(spec.id),
            Some(Item::Scope(choice)) => self.set_query(choice.query),
            Some(Item::Calc(answer)) => self.set_query(answer.result),
            Some(Item::App(position)) => self.launch_app(position),
            Some(Item::Action(action)) if action.confirm => self.open_confirm(action),
            Some(Item::Action(action)) => self.run_quick_action(action),
            None => {}
        }
    }

    pub(super) fn run_command(&mut self, id: &str) {
        match id {
            "help" => self.overlay = Overlay::Help,
            "duplicates" => {
                let groups = index::duplicate_groups(&self.view(), DUPLICATE_SLOTS);
                self.groups = groups;
                self.overlay = Overlay::Duplicates;
            }
            "setup" => self.overlay = Overlay::Setup,
            "grid" => {
                self.grid = !self.grid;
                self.set_query(String::new());
            }
            "home" => {
                let home = PathBuf::from(&self.home);
                self.open_path(&home);
            }
            "clear" => self.set_query(String::new()),
            _ => {}
        }
    }

    pub(super) fn quit(&self) -> ! {
        instance::release(&self.pid_file);
        std::process::exit(0)
    }
}
