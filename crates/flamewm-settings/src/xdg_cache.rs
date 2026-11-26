//! Memoized XDG projections for the Startup and Default Applications pages.
//!
//! Both pages read live files: autostart entries, the installed-application catalog and the
//! `mimeapps.list` defaults. A repaint must not repeat that work, so each projection is kept
//! until its [`XdgStamp`] moves. Stamping costs one `stat` per watched directory or file and
//! reads no contents, so an unchanged page repaints with file reads removed entirely and a
//! changed one reloads exactly once. Writes of this app replace a directory entry, which moves
//! the stamp they touch, and the write paths also clear the cache explicitly.
//!
//! The facade itself is parsed once per Settings process from the environment, and its watched
//! path list is built once with it, so a repaint reuses both instead of re-reading environment
//! variables and re-joining every XDG path on each settle.

use std::rc::Rc;

use flamewm_settings_core::xdg_applications::{
    ApplicationSettings, StartupEntry, XdgStamp, XdgWatch,
};
use flamewm_settings_core::xdg_startup::StartupCandidate;

use crate::{default_apps_page, startup_page};

/// Startup page projection: the visible rows and the picker candidates that exclude them.
#[derive(Debug, Default)]
pub(crate) struct StartupView {
    pub(crate) rows: Vec<StartupEntry>,
    pub(crate) candidates: Vec<StartupCandidate>,
}

/// Last projection per page, keyed by the XDG stamp it was read under.
#[derive(Default)]
pub(crate) struct XdgCache {
    /// Environment-derived facade, parsed once per process instead of per repaint.
    settings: Option<ApplicationSettings>,
    /// Watched paths for `settings`, joined once and shared by every stamp.
    watch: Option<XdgWatch>,
    startup: Option<(XdgStamp, Rc<StartupView>)>,
    defaults: Option<(XdgStamp, Rc<Vec<String>>)>,
}

impl XdgCache {
    /// Environment facade and watched paths, built on first use and reused after that.
    fn prepare(&mut self) {
        if self.settings.is_some() {
            return;
        }
        if let Ok(settings) = ApplicationSettings::from_environment() {
            self.watch = Some(settings.watch());
            self.settings = Some(settings);
        }
    }

    /// Current stamp, or `None` when the environment has no usable `HOME`.
    fn stamp(&mut self) -> Option<XdgStamp> {
        self.prepare();
        let settings = self.settings.as_ref()?;
        let watch = self.watch.as_ref()?;
        Some(settings.stamp_with(watch))
    }

    /// Startup rows and picker candidates, re-read only when the stamp moved.
    pub(crate) fn startup(&mut self) -> Rc<StartupView> {
        let stamp = self.stamp();
        if let (Some(stamp), Some((cached, view))) = (&stamp, &self.startup)
            && cached == stamp
        {
            return Rc::clone(view);
        }
        let Some(stamp) = stamp else {
            return Rc::new(StartupView::default());
        };
        let Some(settings) = self.settings.as_ref() else {
            return Rc::new(StartupView::default());
        };
        let rows = startup_page::rows_with(settings).unwrap_or_default();
        let candidates = settings.startup_candidates_with(&rows).unwrap_or_default();
        let view = Rc::new(StartupView { rows, candidates });
        self.startup = Some((stamp, Rc::clone(&view)));
        view
    }

    /// Default-applications labels, re-read only when the stamp moved.
    pub(crate) fn defaults(&mut self) -> Rc<Vec<String>> {
        let stamp = self.stamp();
        if let (Some(stamp), Some((cached, labels))) = (&stamp, &self.defaults)
            && cached == stamp
        {
            return Rc::clone(labels);
        }
        let Some(stamp) = stamp else {
            return Rc::new(vec![
                "Unavailable".to_owned();
                default_apps_page::GROUPS.len()
            ]);
        };
        let Some(settings) = self.settings.as_ref() else {
            return Rc::new(vec![
                "Unavailable".to_owned();
                default_apps_page::GROUPS.len()
            ]);
        };
        let labels = Rc::new(default_apps_page::labels_with(settings));
        self.defaults = Some((stamp, Rc::clone(&labels)));
        labels
    }

    /// Drops both projections; the next repaint of a page re-reads its files once.
    pub(crate) fn clear(&mut self) {
        self.startup = None;
        self.defaults = None;
    }
}
