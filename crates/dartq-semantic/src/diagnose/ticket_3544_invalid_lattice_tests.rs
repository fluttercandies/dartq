//! The #3544 `Ty::Invalid` wave-1 lattice pins: the new unit variant on the
//! closed [`crate::model::Ty`] enum must read as the oracle's `InvalidType`
//! through every section-6.1-shaped relation the name-level model exposes — the
//! same face `dartq-types::TypeKind::Invalid` carries in the interned
//! lattice — while staying a DISTINCT sentinel from [`Ty::Unknown`] (the
//! Gap face, "no fact"), because nothing in this wave produces `Invalid`
//! and the two answers must never collapse.
//!
//! Oracle contract (upstream Dart SDK source 3.13.7):
//!
//! * `subtype.dart` (`_isSubtypeOf`, :274-276): `InvalidType` on either
//!   side returns `true` — the broken annotation is a subtype of and a
//!   supertype of every type. `type_system.dart` `isAssignableTo`
//!   (:901-904) repeats the face: an invalid source or target accepts.
//!   The name-level mirror is [`Ty::assignable_from`] — `Some(true)`,
//!   never the `None` suppression the `Unknown`/`void` arm answers.
//! * `type.dart` `getDisplayString`/`writeInvalidType`: the broken
//!   annotation displays under its own name — `"InvalidType"`, distinct
//!   from `Unknown`'s empty render and from the `render_slot` splice that
//!   keeps `Unknown => "InvalidType"` for the recovery holes (record
//!   fields, function-shape bounds — unchanged this wave).
//! * `type_system.dart` `isStrictlyNonNullable` (:1921-1951): `Invalid`
//!   answers `false` like `dynamic`/`void`/`Null` — already natural
//!   through the variant's shape (no `Named`-non-nullable arm), pinned
//!   here so a future refactor cannot silently flip it.
//!
//! Zero behavior change is the wave-1 contract: every assert below either
//! exercises `Ty::Invalid` directly (a face no producer emits yet) or
//! re-pins a pre-existing verdict that must not move.

#![allow(clippy::semicolon_if_nothing_returned)]
use crate::model::{ArgPool, Ty};
use dartq_source::StringTable;

/// `int`, `List`, `String` over one shared table/pool pair — the same
/// harness `model::tests::Fix` carries, rebuilt here so the module stands
/// alone.
struct Fix {
    strings: StringTable,
    pool: ArgPool,
}

impl Fix {
    fn new() -> Self {
        Self {
            strings: StringTable::new(),
            pool: ArgPool::default(),
        }
    }

    fn named(&self, text: &str) -> Ty {
        Ty::named(self.strings.intern(text))
    }

    fn render(&self, ty: Ty) -> String {
        ty.render(&self.strings, &self.pool)
    }
}

/// `InvalidType` on either side of the assignability relation accepts —
/// the decided `Some(true)`, never the `Unknown` arm's `None`
/// suppression. Every representative counterpart is pinned: the unit
/// kinds (`Null`, `Dynamic`, `Never`, `Void`), an unwritten `Unknown`,
/// `Invalid` itself (reflexivity), a concrete `Named` head, and a
/// `Named?` head.
#[test]
fn invalid_assigns_to_and_from_every_type() {
    let fix = Fix::new();
    let int = fix.named("int");
    let int_q = Ty::Named {
        name: fix.strings.intern("int"),
        nullable: true,
        args: crate::model::ArgListId::EMPTY,
    };
    let counterparts = [
        Ty::Null,
        Ty::Dynamic,
        Ty::Never,
        Ty::Void,
        Ty::Unknown,
        Ty::Invalid,
        int,
        int_q,
    ];
    for other in counterparts {
        assert_eq!(
            Ty::Invalid.assignable_from(other),
            Some(true),
            "Invalid must accept {other:?} as a source (subtype.dart Left InvalidType)"
        );
        assert_eq!(
            other.assignable_from(Ty::Invalid),
            Some(true),
            "{other:?} must accept Invalid as a source (subtype.dart Right InvalidType)"
        );
    }
}

/// `Unknown` keeps its suppression answer — the two sentinels must never
/// collapse: `Unknown` still answers `None` against every real atom
/// (rule 0c), while `Invalid` on either side decides `Some(true)`
/// (including `Unknown ↔ Invalid` — `subtype.dart`'s InvalidType arm
/// fires before the `Unknown` suppression).
#[test]
fn invalid_and_unknown_stay_distinct_sentinels() {
    assert_ne!(Ty::Invalid, Ty::Unknown);
    // The Gap face still suppresses against the atoms — the wave-1 trap
    // is accidentally re-keying `Unknown` onto the decided arm.
    for other in [Ty::Null, Ty::Dynamic, Ty::Never] {
        assert_eq!(Ty::Unknown.assignable_from(other), None);
        assert_eq!(other.assignable_from(Ty::Unknown), None);
    }
    // Against `Invalid` the InvalidType rule dominates — `Some(true)`,
    // never `None` — mirroring `dartq-types` where rule 0c
    // (`subtype.dart:274-276`) precedes every other arm.
    assert_eq!(Ty::Unknown.assignable_from(Ty::Invalid), Some(true));
    assert_eq!(Ty::Invalid.assignable_from(Ty::Unknown), Some(true));
}

/// `render` — `InvalidType` displays under its own oracle spelling
/// (`writeInvalidType`), never the `Unknown` empty string nor the
/// `render_slot` splice (which still reads `Unknown => "InvalidType"` for
/// the recovery holes — a `Ty::Invalid` field would display the same
/// spelling through the general arm, so the two faces agree on display
/// but diverge on identity).
#[test]
fn invalid_renders_as_invalid_type() {
    let fix = Fix::new();
    assert_eq!(fix.render(Ty::Invalid), "InvalidType");
    // The sentinels' displays stay distinct from the atoms beside them.
    assert_eq!(fix.render(Ty::Unknown), "");
    assert_eq!(fix.render(Ty::Dynamic), "dynamic");
    assert_ne!(fix.render(Ty::Invalid), fix.render(Ty::Dynamic));
}

/// `isStrictlyNonNullable` answers `false` for `Invalid` — the
/// `invalid_null_aware_operator` receiver test reads the broken
/// annotation as a top face, never a strictly-non-nullable one.
#[test]
fn invalid_is_not_strictly_non_nullable() {
    let fix = Fix::new();
    assert!(!Ty::Invalid.is_strictly_non_nullable());
    // The neighboring atoms keep their answers — the variant must not
    // perturb the existing table.
    assert!(!Ty::Unknown.is_strictly_non_nullable());
    assert!(!Ty::Dynamic.is_strictly_non_nullable());
    assert!(!Ty::Void.is_strictly_non_nullable());
    assert!(!Ty::Null.is_strictly_non_nullable());
    assert!(Ty::Never.is_strictly_non_nullable());
    assert!(fix.named("int").is_strictly_non_nullable());
}
