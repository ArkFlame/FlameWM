//! Pure range (slider/spin) model. Renderer-neutral; native adapters map
//! pointer/keyboard input to [`RangeAction`] and apply the returned value.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeSpec {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub value: f32,
}

impl RangeSpec {
    #[must_use]
    pub fn new(min: f32, max: f32, step: f32, value: f32) -> Self {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        let mut spec = Self {
            min: lo,
            max: hi,
            step: step.max(0.0),
            value,
        };
        spec.value = spec.clamp_value(value);
        spec
    }

    #[must_use]
    pub fn clamp_value(self, value: f32) -> f32 {
        if !value.is_finite() {
            return self.value.clamp(self.min, self.max);
        }
        value.clamp(self.min, self.max)
    }

    #[must_use]
    pub fn snap(self, value: f32) -> f32 {
        let clamped = self.clamp_value(value);
        if self.step <= 0.0 {
            return clamped;
        }
        let steps = ((clamped - self.min) / self.step).round();
        self.clamp_value(self.min + steps * self.step)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RangeState {
    pub value: f32,
    pub dragging: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RangeAction {
    SetValue(f32),
    StepUp,
    StepDown,
    BeginDrag,
    EndDrag,
}

impl RangeState {
    #[must_use]
    pub fn apply(self, spec: RangeSpec, action: RangeAction) -> Self {
        match action {
            RangeAction::SetValue(value) => Self {
                value: spec.snap(value),
                ..self
            },
            RangeAction::StepUp => Self {
                value: spec.snap(self.value + spec.step.max(f32::EPSILON)),
                ..self
            },
            RangeAction::StepDown => Self {
                value: spec.snap(self.value - spec.step.max(f32::EPSILON)),
                ..self
            },
            RangeAction::BeginDrag => Self {
                dragging: true,
                ..self
            },
            RangeAction::EndDrag => Self {
                dragging: false,
                ..self
            },
        }
    }

    /// Map a pointer coordinate on a [`track_start`, `track_start+track_len`]
    /// track to a snapped range value.
    #[must_use]
    pub fn value_from_pointer(
        spec: RangeSpec,
        track_start: f32,
        track_len: f32,
        pointer: f32,
    ) -> f32 {
        if track_len <= 0.0 {
            return spec.clamp_value(spec.value);
        }
        let ratio = ((pointer - track_start) / track_len).clamp(0.0, 1.0);
        spec.snap(spec.min + ratio * (spec.max - spec.min))
    }
}
