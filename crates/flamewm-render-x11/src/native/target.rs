//! Transactional geometry commit + presentation state machine.
//! Pure state logic lives here; the X11 side-effects stay in `app/`.
//! [`NativeSurfaceTarget`] owns the scene/window visual resource set and
//! exposes transactional create/drop/resize plans.

use crate::native::surface_format::SurfaceAlphaMode;

/// Native presentation lifecycle. Only forward transitions are legal;
/// `Mapped` requires a prior `MapNotify` (see `note_mapped`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentationState {
    #[default]
    CreatedHidden,
    Projected,
    GeometryCommitted,
    Painted,
    Mapped,
    Presented,
}

impl PresentationState {
    /// Attempt a forward transition. Rejects skips and any backward move.
    pub fn advance_to(self, next: Self) -> Result<Self, String> {
        let order = |s: Self| match s {
            Self::CreatedHidden => 0,
            Self::Projected => 1,
            Self::GeometryCommitted => 2,
            Self::Painted => 3,
            Self::Mapped => 4,
            Self::Presented => 5,
        };
        if order(next) == order(self) + 1 {
            Ok(next)
        } else {
            Err(presenter_error(&format!(
                "invalid presentation transition {self:?} -> {next:?}"
            )))
        }
    }

    /// Record a MapNotify: only Painted -> Mapped is legal.
    pub fn note_mapped(self) -> Result<Self, String> {
        self.advance_to(Self::Mapped)
    }
}

/// Error constructor that never swallows context on first present.
pub fn presenter_error(detail: &str) -> String {
    format!("native presenter: {detail}")
}

/// Transactional geometry commit plan: retained size first, then backbuffer,
/// retarget, resize, shape, repaint, flush. Callers apply each step in order
/// and abort (no partial window resize) when any step fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryCommit {
    pub width: u32,
    pub height: u32,
}

impl GeometryCommit {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let (w, h) = (width.max(1), height.max(1));
        if w > 16384 || h > 16384 {
            return Err(presenter_error("geometry commit exceeds 16384px limit"));
        }
        Ok(Self {
            width: w,
            height: h,
        })
    }
}

/// Resource-owner record for one native ARGB scene surface.
/// Holds the visual/depth/colormap/pixmap/gc/picture identity plus size and
/// alpha mode; transactional helpers plan create/drop/resize so `app/` applies
/// steps in order and aborts without a partial window resize on failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeSurfaceTarget {
    pub scene: u64,
    pub visual_id: u64,
    pub depth: u32,
    pub colormap: u64,
    pub pixmap: u64,
    pub gc: u64,
    pub picture: u64,
    pub width: u32,
    pub height: u32,
    pub alpha_mode: SurfaceAlphaMode,
}

impl NativeSurfaceTarget {
    pub fn create_plan(
        scene: u64,
        visual_id: u64,
        depth: u32,
        alpha_mode: SurfaceAlphaMode,
        commit: GeometryCommit,
    ) -> Self {
        Self {
            scene,
            visual_id,
            depth,
            colormap: 0,
            pixmap: 0,
            gc: 0,
            picture: 0,
            width: commit.width,
            height: commit.height,
            alpha_mode,
        }
    }

    /// After native allocation succeeds, record handles transactionally:
    /// all-or-nothing; zero handles are rejected.
    pub fn bind_handles(
        &mut self,
        colormap: u64,
        pixmap: u64,
        gc: u64,
        picture: u64,
    ) -> Result<(), String> {
        if colormap == 0 || pixmap == 0 || gc == 0 {
            return Err(presenter_error("surface bind requires non-zero handles"));
        }
        // Composited path also requires a picture; opaque/shape paths may
        // carry picture == 0 legitimately.
        if self.alpha_mode == SurfaceAlphaMode::CompositedArgb32 && picture == 0 {
            return Err(presenter_error("composited surface requires a picture"));
        }
        self.colormap = colormap;
        self.pixmap = pixmap;
        self.gc = gc;
        self.picture = picture;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn is_live(&self) -> bool {
        self.pixmap != 0 && self.gc != 0 && self.colormap != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::surface_format::SurfaceAlphaMode;

    #[test]
    fn rejects_skipped_and_backward_transitions() {
        let s = PresentationState::CreatedHidden;
        assert!(s.advance_to(PresentationState::GeometryCommitted).is_err());
        let s = PresentationState::Painted;
        assert!(s.advance_to(PresentationState::GeometryCommitted).is_err());
        assert_eq!(
            s.advance_to(PresentationState::Mapped).unwrap(),
            PresentationState::Mapped
        );
    }

    #[test]
    fn mapped_requires_painted_first() {
        assert!(PresentationState::CreatedHidden.note_mapped().is_err());
        assert!(PresentationState::Painted.note_mapped().is_ok());
    }

    #[test]
    fn commit_clamps_and_rejects_huge() {
        assert_eq!(
            GeometryCommit::new(0, 0).unwrap(),
            GeometryCommit {
                width: 1,
                height: 1
            }
        );
        assert!(GeometryCommit::new(20000, 10).is_err());
    }

    #[test]
    fn surface_bind_rejects_zero_and_requires_picture_when_composited() {
        let commit = GeometryCommit::new(8, 8).unwrap();
        let mut t =
            NativeSurfaceTarget::create_plan(1, 2, 32, SurfaceAlphaMode::CompositedArgb32, commit);
        assert!(!t.is_live());
        assert!(t.bind_handles(1, 2, 3, 0).is_err());
        assert!(t.bind_handles(0, 2, 3, 4).is_err());
        t.bind_handles(1, 2, 3, 4).unwrap();
        assert!(t.is_live());
    }
}
