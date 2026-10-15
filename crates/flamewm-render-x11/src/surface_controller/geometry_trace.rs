use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// Correlated surface-geometry trace transaction. One id traces one
/// placement: anchor rect -> intrinsic size -> fitted root rect ->
/// SurfaceRuntime request -> SurfaceController request -> native
/// XMoveResizeWindow request -> observed Configure/Map root rect ->
/// retained origin. Hot path is in-memory only (no X roundtrip); the
/// observed stage is filled solely by `note_observed_rect` from an
/// already-delivered Configure/Map event or a debug-only reply probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryTraceTxn(u64);

impl GeometryTraceTxn {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// One stage of a geometry trace. Stored in-memory; formatted only when
/// `FLAMEWM_GEOM_TRACE=1` and emitted through `flamewm-debug` when
/// `FLAMEWM_DEBUG=1`. Never roundtrips X on the hot path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryTraceStage {
    pub txn: u64,
    pub stage: &'static str,
    pub surface: u64,
    pub role: &'static str,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// In-memory ring of geometry trace stages, shared by the trace contract.
/// Single owner: `flamewm-render-x11`. UI/shell layers only record through
/// the typed `GeometryTrace` handle and read via `snapshot_for_txn`.
#[derive(Debug, Default)]
pub struct GeometryTraceLog {
    stages: Vec<GeometryTraceStage>,
}

impl GeometryTraceLog {
    pub fn record(&mut self, stage: GeometryTraceStage) {
        self.stages.push(stage);
        if self.stages.len() > 512 {
            let excess = self.stages.len() - 512;
            self.stages.drain(..excess);
        }
    }

    pub fn snapshot_for_txn(&self, txn: u64) -> Vec<GeometryTraceStage> {
        self.stages
            .iter()
            .copied()
            .filter(|stage| stage.txn == txn)
            .collect()
    }

    /// Debug/roundtrip-only observed-rect probe for one native window.
    /// Called off the hot path only (debug tooling); production geometry
    /// commits never issue a synchronous X roundtrip.
    pub fn note_observed(
        &mut self,
        txn: u64,
        surface: u64,
        role: &'static str,
        rect: (i32, i32, i32, i32),
    ) {
        self.record(GeometryTraceStage {
            txn,
            stage: "observed",
            surface,
            role,
            x: rect.0,
            y: rect.1,
            width: rect.2,
            height: rect.3,
        });
    }
}

pub(super) fn geometry_trace_log() -> &'static std::sync::Mutex<GeometryTraceLog> {
    static LOG: std::sync::OnceLock<std::sync::Mutex<GeometryTraceLog>> =
        std::sync::OnceLock::new();
    LOG.get_or_init(|| std::sync::Mutex::new(GeometryTraceLog::default()))
}

pub(super) fn next_geometry_txn() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let txn = NEXT.fetch_add(1, Ordering::Relaxed);
    if txn == 0 {
        NEXT.fetch_add(1, Ordering::Relaxed)
    } else {
        txn
    }
}

/// Typed trace handle threaded through shell -> SurfaceRuntime ->
/// SurfaceController -> native. Source of truth for one correlated
/// placement; carries the txn id and the surface role label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryTrace {
    pub txn: u64,
    pub surface: u64,
    pub role: &'static str,
}

impl GeometryTrace {
    pub fn begin(role: &'static str) -> Self {
        Self {
            txn: next_geometry_txn(),
            surface: 0,
            role,
        }
    }

    pub fn for_surface(self, surface: u64) -> Self {
        Self { surface, ..self }
    }

    pub fn txn(&self) -> GeometryTraceTxn {
        GeometryTraceTxn(self.txn)
    }

    pub(super) fn emit(&self, stage: &'static str, rect: (i32, i32, i32, i32)) {
        if let Ok(mut log) = geometry_trace_log().lock() {
            log.record(GeometryTraceStage {
                txn: self.txn,
                stage,
                surface: self.surface,
                role: self.role,
                x: rect.0,
                y: rect.1,
                width: rect.2,
                height: rect.3,
            });
        }
        if std::env::var_os("FLAMEWM_GEOM_TRACE").is_some_and(|v| v == "1") {
            eprintln!(
                "geom-trace txn={} stage={} surface={} role={} rect={},{},{},{}",
                self.txn, stage, self.surface, self.role, rect.0, rect.1, rect.2, rect.3,
            );
        }
    }

    pub fn anchor(&self, rect: (i32, i32, i32, i32)) {
        self.emit("anchor", rect);
    }

    pub fn intrinsic(&self, size: (i32, i32)) {
        self.emit("intrinsic", (0, 0, size.0, size.1));
    }

    pub fn fitted(&self, rect: (i32, i32, i32, i32)) {
        self.emit("fitted", rect);
    }

    pub fn ui_request(&self, rect: (i32, i32, i32, i32)) {
        self.emit("ui-request", rect);
    }

    pub fn native_request(&self, rect: (i32, i32, i32, i32)) {
        self.emit("native-request", rect);
    }

    pub fn retained(&self, rect: (i32, i32, i32, i32)) {
        self.emit("retained", rect);
    }

    /// Debug-only observed stage (Configure/Map reply or async event).
    /// Never called on the production hot path.
    pub fn observed(&self, rect: (i32, i32, i32, i32)) {
        if let Ok(mut log) = geometry_trace_log().lock() {
            log.note_observed(self.txn, self.surface, self.role, rect);
        }
    }
}

/// Snapshot of all recorded stages for one transaction id. Used by tests
/// and debug tooling to prove one correlated id traces the full chain.
pub fn geometry_trace_snapshot(txn: u64) -> Vec<GeometryTraceStage> {
    geometry_trace_log()
        .lock()
        .map(|log| log.snapshot_for_txn(txn))
        .unwrap_or_default()
}

impl SurfaceController {
    /// Debug/roundtrip-only observed-rect probe for one surface. Off the
    /// hot path: reads retained state (no X sync). Real async
    /// ConfigureNotify/MapNotify reconciliation stays in `app/events.rs`.
    pub fn debug_observed_rect(
        &self,
        id: SurfaceId,
        trace: GeometryTrace,
    ) -> Result<(i32, i32, i32, i32), String> {
        let rect = self.surface_device_rect(id)?;
        let observed = (
            rect.x as i32,
            rect.y as i32,
            rect.width as i32,
            rect.height as i32,
        );
        trace.for_surface(id.get()).observed(observed);
        Ok(observed)
    }

    /// DEBUG-ONLY actual root-rect probe after present (X native owner).
    /// Normal mode: refuses up front, zero extra X sync. Debug/test mode:
    /// exactly one retained root-rect read (origin + size, no X roundtrip,
    /// no layout recompute), then records the `observed` trace stage.
    /// Call once per popup show.
    pub fn debug_probe_root_rect(
        &self,
        id: SurfaceId,
        trace: GeometryTrace,
    ) -> Result<(i32, i32, i32, i32), String> {
        if !debug_enabled() && !cfg!(test) {
            return Err("debug-only probe refused outside FLAMEWM_DEBUG=1".to_string());
        }
        self.debug_observed_rect(id, trace)
    }

    /// DEBUG/TEST-ONLY stacking canary over retained role order.
    /// Checks bottom-to-top creation order satisfies desktop < normal <
    /// dock < popup via [`stack_order_ok`]. Zero X sync in all modes;
    /// a live QueryTree roundtrip stays in the nested-X harness, never on
    /// this path. Returns `Ok(true)` when the order holds, `Ok(false)`
    /// on inversion. Refuses (zero work) outside debug/test. Never
    /// reorders here; the caller re-uses the existing map/raise owner to
    /// repair and reports.
    pub fn debug_stack_canary(&self) -> Result<bool, String> {
        if !debug_enabled() && !cfg!(test) {
            return Err("debug-only canary refused outside FLAMEWM_DEBUG=1".to_string());
        }
        let mut ids: Vec<SurfaceId> = self.surfaces.keys().copied().collect();
        ids.sort_by_key(|id| id.get());
        let roles: Vec<SurfaceRole> = ids
            .iter()
            .filter_map(|id| self.surfaces.get(id).map(|instance| instance.role))
            .collect();
        Ok(stack_order_ok(&roles))
    }

}
