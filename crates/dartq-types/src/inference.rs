//! Generic type inference: the merged-constraint representation and the
//! official two-phase choice order.
//!
//! Behavior anchors (upstream Dart SDK source): the constraint store is
//! `MergedTypeConstraint` (`pkg/_fe_analyzer_shared/lib/src/type_inference/
//! type_constraint.dart` — `mergeInTypeSchemaLower` joins lowers by the
//! schema LUB, `mergeInTypeSchemaUpper` joins uppers by the schema GLB), and
//! the choice order is `chooseTypeFromConstraint`
//! (`type_analyzer_operations.dart:1015-1075`): prefer a known lower bound,
//! then a known upper; otherwise take the partial bound, grounded through
//! its least/greatest closure in the final phase. The preliminary
//! (downward) phase calls this ungrounded (`inferTypeParameterFromContext`),
//! the final (upward) phase grounded (`inferTypeParameterFromAll`), and a
//! type fixed by the downward phase is never revisited
//! (`_typesInferredSoFar`, `generic_inferrer.dart:98-98/:150-165`).
//!
//! The core is generic over a [`InferenceLattice`] because its two real
//! universes live in different crates: the semantic layer's written-type
//! model (`Ty`) drives today's invocation diagnostics, and the interned
//! [`crate::TypeId`] lattice is the M2 target. Neither type can move here
//! without a dependency cycle, so the decision order — the part that must
//! mirror the oracle — lives here, and each universe supplies its own
//! lattice operations.

/// The lattice operations the solve needs, supplied per type universe.
pub trait InferenceLattice {
    /// The type representation.
    type Ty: Copy + Eq;

    /// Whether `ty` is fully known — free of the inference-unknown
    /// sentinel, at the root and inside every argument (the official
    /// `isKnownType`, `type_analyzer_operations.dart:1018`).
    fn is_known(&self, ty: Self::Ty) -> bool;

    /// Whether `ty` is the unknown sentinel itself (`UnknownInferredType`).
    fn is_unknown(&self, ty: Self::Ty) -> bool;

    /// Whether the sentinel occurs anywhere inside `ty`'s structure.
    fn mentions_unknown(&self, ty: Self::Ty) -> bool;

    /// The schema LUB (`typeSchemaLub`) — lower-bound joins.
    fn lub(&self, a: Self::Ty, b: Self::Ty) -> Self::Ty;

    /// The schema GLB (`typeSchemaGlb`) — upper-bound joins. The official
    /// merge consults the *existing* upper first
    /// (`glbInternal` → `greatestLowerBound`, `type_system.dart:663`), so a
    /// schema-unknown upper swallows a known one (`List<_>` ⊓ `List<int>`
    /// stays `List<_>`, the issue63883 shape).
    fn glb(&self, a: Self::Ty, b: Self::Ty) -> Self::Ty;

    /// `greatestClosureOfSchema` — the grounded read of a partial upper
    /// (`type_analyzer_operations.dart:1057-1059`): every sentinel closes
    /// to its largest inhabitants.
    fn greatest_closure(&self, ty: Self::Ty) -> Self::Ty;

    /// `leastClosureOfSchema` — the grounded read of a partial lower
    /// (`type_analyzer_operations.dart:1054-1056`): every sentinel closes
    /// to its smallest inhabitants.
    fn least_closure(&self, ty: Self::Ty) -> Self::Ty;
}

/// The per-type-variable constraint store: one merged lower and upper bound
/// (`MergedTypeConstraint`, type_constraint.dart:38-198).
#[derive(Clone, Debug, Default)]
pub struct MergedTypeConstraint<T> {
    lower: Option<T>,
    upper: Option<T>,
}

impl<T> MergedTypeConstraint<T> {
    /// A fresh, unconstrained store.
    pub fn new() -> Self {
        Self {
            lower: None,
            upper: None,
        }
    }
}

impl<T: Copy> MergedTypeConstraint<T> {
    /// The merged lower bound, if any constraint contributed one.
    pub fn lower(&self) -> Option<T> {
        self.lower
    }

    /// The merged upper bound, if any constraint contributed one.
    pub fn upper(&self) -> Option<T> {
        self.upper
    }

    /// `mergeInTypeSchemaLower` (type_constraint.dart:174-185): the lower
    /// bound joins by the schema LUB.
    pub fn merge_lower<L: InferenceLattice<Ty = T>>(&mut self, ty: T, lattice: &L) {
        self.lower = Some(match self.lower {
            None => ty,
            Some(lower) => lattice.lub(lower, ty),
        });
    }

    /// `mergeInTypeSchemaUpper` (type_constraint.dart:187-198): the upper
    /// bound joins by the schema GLB.
    pub fn merge_upper<L: InferenceLattice<Ty = T>>(&mut self, ty: T, lattice: &L) {
        self.upper = Some(match self.upper {
            None => ty,
            Some(upper) => lattice.glb(upper, ty),
        });
    }
}

/// `chooseTypeFromConstraint` (`type_analyzer_operations.dart:1015-1075`)
/// in the *preliminary* posture: `grounded: false`, the downward phase's
/// call (`inferTypeParameterFromContext`, :1277-1345). Partial bounds are
/// taken as written — the schema flows into argument contexts.
#[must_use]
pub fn choose_preliminary_type<T: Copy, L: InferenceLattice<Ty = T>>(
    constraint: &MergedTypeConstraint<T>,
    lattice: &L,
) -> T {
    choose_type_from_constraint(constraint, lattice, false, false)
}

/// `chooseTypeFromConstraint` in the *final* posture: `grounded: true`, the
/// upward phase's call (`inferTypeParameterFromAll`, :1207-1266). Partial
/// bounds ground through their closures; a lower-bound `?` closes down, an
/// upper-bound `?` closes up.
#[must_use]
pub fn choose_final_type<T: Copy, L: InferenceLattice<Ty = T>>(
    constraint: &MergedTypeConstraint<T>,
    lattice: &L,
) -> T {
    choose_type_from_constraint(constraint, lattice, true, false)
}

/// The contravariant arm of the same choice
/// (`type_analyzer_operations.dart:1041-1074`): a contravariant position
/// flips the preference order and the closures. Nothing in today's
/// consumers declares contravariant parameters; the arm keeps the port
/// exact against the source.
#[must_use]
pub fn choose_type_contravariant<T: Copy, L: InferenceLattice<Ty = T>>(
    constraint: &MergedTypeConstraint<T>,
    lattice: &L,
    grounded: bool,
) -> T {
    choose_type_from_constraint(constraint, lattice, grounded, true)
}

/// The shared rule table; the arm order is the source's.
fn choose_type_from_constraint<T: Copy, L: InferenceLattice<Ty = T>>(
    constraint: &MergedTypeConstraint<T>,
    lattice: &L,
    grounded: bool,
    contravariant: bool,
) -> T {
    let (first, second) = if !contravariant {
        (constraint.lower, constraint.upper)
    } else {
        (constraint.upper, constraint.lower)
    };
    // Prefer the known bound, if any (:1021-1027 / :1043-1049).
    if let Some(bound) = first
        && lattice.is_known(bound)
    {
        return bound;
    }
    if let Some(bound) = second
        && lattice.is_known(bound)
    {
        return bound;
    }
    // Otherwise take whatever bound has partial information (:1028-1040 /
    // :1050-1062); the grounded phase closes it — a first-position bound
    // through its least closure, a second-position one through its
    // greatest (the arms swap for a contravariant position).
    if let Some(bound) = first
        && !lattice.is_unknown(bound)
    {
        return match (grounded, contravariant) {
            (false, _) => bound,
            (true, false) => lattice.least_closure(bound),
            (true, true) => lattice.greatest_closure(bound),
        };
    }
    if let Some(bound) = second
        && !lattice.is_unknown(bound)
    {
        return match (grounded, contravariant) {
            (false, _) => bound,
            (true, false) => lattice.greatest_closure(bound),
            (true, true) => lattice.least_closure(bound),
        };
    }
    // Both bounds absent or pure unknowns (:1063-1066): the unknown stands.
    first.unwrap_or_else(|| second.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-point lattice over `i32`: 0 is the unknown sentinel, 1 is a
    /// schema-unknown (`_`), 2+ are known types ordered by value; lub takes
    /// the max, glb prefers an unknown-carrying operand then the min.
    struct IntLattice;

    impl InferenceLattice for IntLattice {
        type Ty = i32;
        fn is_known(&self, ty: i32) -> bool {
            ty >= 2
        }
        fn is_unknown(&self, ty: i32) -> bool {
            ty == 0
        }
        fn mentions_unknown(&self, ty: i32) -> bool {
            ty < 2
        }
        fn lub(&self, a: i32, b: i32) -> i32 {
            a.max(b)
        }
        fn glb(&self, a: i32, b: i32) -> i32 {
            if a < 2 {
                a
            } else if b < 2 {
                b
            } else {
                a.min(b)
            }
        }
        fn greatest_closure(&self, ty: i32) -> i32 {
            if ty == 0 || ty == 1 { 9 } else { ty }
        }
        fn least_closure(&self, ty: i32) -> i32 {
            if ty == 0 || ty == 1 { 2 } else { ty }
        }
    }

    /// A known lower wins in both phases (:1021-1023).
    #[test]
    fn known_lower_wins() {
        let lattice = IntLattice;
        let mut c = MergedTypeConstraint::new();
        c.merge_lower(3, &lattice);
        c.merge_lower(5, &lattice);
        assert_eq!(choose_preliminary_type(&c, &lattice), 5);
        assert_eq!(choose_final_type(&c, &lattice), 5);
    }

    /// A known upper answers only without a known lower (:1024-1026).
    #[test]
    fn known_upper_after_lower() {
        let lattice = IntLattice;
        let mut c = MergedTypeConstraint::new();
        c.merge_upper(4, &lattice);
        assert_eq!(choose_preliminary_type(&c, &lattice), 4);
        c.merge_lower(0, &lattice);
        c.merge_upper(7, &lattice);
        assert_eq!(choose_final_type(&c, &lattice), 7);
    }

    /// The partial-upper grounding: the downward phase keeps the schema,
    /// the upward phase closes it — the issue63883 shape (`List<_>` upper
    /// grounds to `List<Object?>` upward, stays the schema downward).
    #[test]
    fn partial_upper_grounds_through_closure() {
        let lattice = IntLattice;
        let mut c = MergedTypeConstraint::new();
        // The GLB merge: an unknown-carrying existing upper swallows the
        // known one (the official consults the existing upper first).
        c.merge_upper(1, &lattice);
        c.merge_upper(5, &lattice);
        assert_eq!(c.upper(), Some(1));
        assert_eq!(choose_preliminary_type(&c, &lattice), 1);
        assert_eq!(choose_final_type(&c, &lattice), 9);
    }

    /// The contravariant arm flips the preference (:1043-1049).
    #[test]
    fn contravariant_prefers_upper_first() {
        let lattice = IntLattice;
        let mut c = MergedTypeConstraint::new();
        c.merge_lower(3, &lattice);
        c.merge_upper(7, &lattice);
        assert_eq!(choose_type_contravariant(&c, &lattice, true), 7);
    }

    /// Unconstrained: the unknown stands (:1063-1066).
    #[test]
    fn unconstrained_stays_unknown() {
        let lattice = IntLattice;
        let c = MergedTypeConstraint::<i32>::new();
        assert_eq!(choose_preliminary_type(&c, &lattice), 0);
    }
}
