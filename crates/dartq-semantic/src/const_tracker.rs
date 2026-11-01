//! The constant-pattern tracker: the in-flight constant pattern's armed
//! static type and anchor (seam 2 of the diagnose engine extraction,
//! ticket #1050).
//!
//! [`ConstTracker`] owns the fold state the engine's literal events arm and
//! the constant-pattern end event consumes. The classification leaf lives
//! on [`crate::switch_tracker::SwitchTracker`] (`pending_const`, seam 1);
//! the engine coordinates the two trackers at the `begin`/`end_constant_
//! pattern` crossings, and the enum-tail classification and the never-
//! matches verdict stay engine-side (they read the engine's row stack and
//! report machinery).

use crate::model::Ty;
use dartq_source::SourceRange;

/// The in-flight constant pattern's armed type and anchor: one engine owns
/// one tracker; the state was engine fields before the extraction
/// (behavior byte-identical).
pub(crate) struct ConstTracker {
    /// The in-flight constant pattern's static type (the
    /// `hasPrimitiveEquality` shapes: int/double/String/bool/Null; enum
    /// tails classify at the pattern's end from the row stack).
    pub(crate) pending_const_ty: Option<Ty>,
    /// The anchor the `constant_pattern_never_matches_value_type`
    /// diagnostic attaches to.
    pub(crate) pending_const_at: Option<SourceRange>,
    /// Armings inside the current pattern window. A single literal IS the
    /// pattern's expression, so its type answers `value.type`; two or more
    /// means the expression is a composite (`1 == 2`, `'a' 'b'`, `{1, 2}`)
    /// whose own static type the last arm cannot spell — the recovered
    /// eval tail in `class_static_const_tail_ty` answers it instead
    /// (upstream `visitConstantPattern` reads the DartObject's
    /// `value.type`, never an operand's).
    arms: u32,
}

impl ConstTracker {
    pub(crate) fn new() -> Self {
        ConstTracker {
            pending_const_ty: None,
            pending_const_at: None,
            arms: 0,
        }
    }

    /// Disarms a stale arming: `begin_constant_pattern` clears the state so
    /// sibling list/map elements' literal armings die at the bracket.
    pub(crate) fn clear(&mut self) {
        self.pending_const_ty = None;
        self.pending_const_at = None;
        self.arms = 0;
    }

    /// Arms the literal's static type and anchor (the engine gates the arm
    /// on the open-pattern depth).
    pub(crate) fn arm(&mut self, ty: Ty, at: SourceRange) {
        self.arms += 1;
        self.pending_const_ty = Some(ty);
        self.pending_const_at = Some(at);
    }

    /// Takes the armed type (the pattern's end consumes it). A composite
    /// constant — more than one literal arming in the window — has no
    /// single armed type: the expression's own recovered type answers from
    /// the eval tail, so `None` falls through to it.
    pub(crate) fn take_ty(&mut self) -> Option<Ty> {
        if self.arms > 1 {
            return None;
        }
        self.pending_const_ty.take()
    }

    /// Takes the armed anchor (the pattern's end consumes it).
    pub(crate) fn take_at(&mut self) -> Option<SourceRange> {
        self.pending_const_at.take()
    }
}
