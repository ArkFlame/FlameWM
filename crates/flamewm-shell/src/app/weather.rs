//! Weather tick, feed publication.

use super::*;

impl ShellLoop {
    /// Weather tick: no timers and no fetch while disabled. Projects the tray and pushes
    /// the feed only when the scheduler changed or a settings snapshot arrived.
    pub(super) fn tick_weather(&mut self, runtime: &mut SurfaceRuntime) {
        let changed = self.weather.tick(
            std::time::Instant::now(),
            flamewm_shell::weather_host::spawn_weather_fetch,
        );
        if !(changed || self.weather_feed_pending) {
            return;
        }
        let feed = self.weather.feed();
        let units = feed.settings.units;
        let shown =
            crate::entry::settings_projection::taskbar_items(self.settings_cache.as_ref()).weather;
        // All native monitor panels own independent retained documents. A weather
        // tick must update every panel, not just the primary surface (a stale
        // second monitor otherwise keeps the previous condition/temperature).
        let mut tray_ok = true;
        for panel in self.surfaces.panel_handles() {
            let projected = runtime.with_document(panel, |document| {
                projection::project_weather_tray(document, &feed.state, units, shown)
            });
            if let Err(error) = &projected {
                eprintln!("flamewm-shell: weather tray projection failed: {error:?}");
                tray_ok = false;
            }
            if let Err(error) = runtime.redraw(panel) {
                eprintln!("flamewm-shell: weather tray redraw failed: {error:?}");
                tray_ok = false;
            }
        }
        self.push_weather_feed();
        self.publish_weather_feed();
        if !tray_ok {
            self.weather_feed_pending = true;
        }
    }

    /// Publishes the same encoded feed to the control host, which caches it for the desktop
    /// widget. Nonblocking submit; the host bumps its revision only when the payload changes.
    pub(super) fn publish_weather_feed(&self) {
        self.submit(flamewm_control_core::ControlRequest::PublishWeatherFeed {
            payload: self.weather.feed().encode(),
        });
    }

    /// Nonblocking feed push. A full pipe re-arms the pending flag for the next tick;
    /// no running popup host is not an error (its first OPEN carries the feed).
    pub(super) fn push_weather_feed(&mut self) {
        match self.quick_control.push_weather(self.weather.feed()) {
            Ok(()) => self.weather_feed_pending = false,
            Err(flamewm_shell::quick_controls::SupervisorError::WouldBlock) => {
                self.weather_feed_pending = true;
            }
            Err(error) => eprintln!("flamewm-shell: weather feed not sent (nonfatal): {error}"),
        }
    }
}
