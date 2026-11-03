//! #3857 pins — a promoted nullable-record receiver resolves its declared
//! fields: the stand-down that clears `receiver_nullable` (`r1 != null`,
//! `r1!`) must hand the shape check the NON-NULL face too, because the
//! field consult rejects a nullable record outright (`record_field_ty`'s
//! `nullable && !null_aware -> None`). The oracle reads one promoted
//! `receiverType` for both facts:
//! `type_property_resolver.dart` derives `isNullable` from it (:95-99) and
//! reaches its `RecordTypeImpl` field arm only after the nullable tail
//! (:224-229), so `typedef R1 = (int, String)?` under a guard reads
//! `$1`/`$2` exactly as the non-null spelling does.
//!
//! Oracle provenance (`dart analyze` 3.13.7, the census families
//! `Records/type_promotion_A01_t01` and `_t03`):
//! * guarded alias / guarded named field — silent;
//! * the UNGUARDED twin — one `UNCHECKED_USE_OF_NULLABLE_VALUE` at the
//!   selector, never an `undefined_getter` (the gate this ticket adds);
//! * `?.`-unwrapped alias field — silent (the fold corroboration);
//! * a guarded access to an ABSENT field — `UNDEFINED_GETTER` on the alias;
//! * a guarded WRITE to a record field — `UNDEFINED_SETTER` on the alias
//!   (records are immutable upstream; no setter element exists).
//!
//! Known residual, deliberately not pinned here: a receiver whose DECLARED
//! spelling is a nullability-suffixed record type (`(String,)? r4`,
//! `(int, {String s})? r4` — the same cases' :45 / :46-47 rows) keeps
//! reporting `undefined_getter`, because the stand-down needs a promotion
//! fact (`promotions.promoted`) the flow layer stages for alias- and
//! interface-typed reads only — an un-promoted spelled record answers none,
//! as its absence of `unnecessary_null_comparison` corroborates. That gap
//! lives in the promotion staging, not in this file's shape check.

use super::test_rig::{report_ops, rows};

/// The member-face codes a shape receiver answers — the family these pins
/// hold to the oracle (the unchecked-nullable tail plus the access-intent
/// trio).
const MEMBER_FACE_CODES: [&str; 5] = [
    "unchecked_use_of_nullable_value",
    "undefined_getter",
    "undefined_setter",
    "undefined_method",
    "undefined_operator",
];

/// The member-face rows of `report` alone — `code @offset: message` each.
fn member_rows(report: &super::FileReport) -> Vec<String> {
    rows(report)
        .into_iter()
        .filter(|row| {
            let code = row.split(' ').next().unwrap_or_default();
            MEMBER_FACE_CODES.contains(&code)
        })
        .collect()
}

/// A promoted alias-to-record receiver reads its positional fields: the
/// guarded `r1.$1`/`r1.$2` over `typedef R1 = (int, String)?` (census
/// `Records/type_promotion_A01_t01` :22-23; oracle silent). The stand-down
/// cleared the nullable flag but kept handing the nullable shape to the
/// field consult, which rejected it and reported `undefined_getter` on
/// `'R1'`.
#[test]
fn guarded_alias_record_reads_positional_fields() {
    let report = report_ops(
        "typedef R1 = (int, String)?;\n\
         void f(R1 r1) {\n\
           if (r1 != null) {\n\
             r1.$1;\n\
             r1.$2;\n\
           }\n\
         }\n",
    );
    assert!(member_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// A promoted alias-to-record receiver reads named fields as well
/// (`typedef R1 = (int, {String s})?` — the census
/// `Records/type_promotion_A01_t03` :22-23 shape, whose own spelling names
/// the positional field too; oracle silent).
#[test]
fn guarded_alias_record_reads_named_field() {
    let report = report_ops(
        "typedef R1 = (int, {String s})?;\n\
         void f(R1 r1) {\n\
           if (r1 != null) {\n\
             r1.$1;\n\
             r1.s;\n\
           }\n\
         }\n",
    );
    assert!(member_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// The gate: the UNGUARDED twin keeps the nullable face, so its row is the
/// shared unchecked dereference at the selector (oracle: one
/// `UNCHECKED_USE_OF_NULLABLE_VALUE`, "The property '$1' can't be
/// unconditionally accessed because the receiver can be 'null'."). Passing
/// the non-null face here would trade that row for a spurious
/// `undefined_getter`.
#[test]
fn unguarded_alias_record_keeps_the_unchecked_face() {
    let report = report_ops(
        "typedef R1 = (int, String)?;\n\
         void f(R1 r1) {\n\
           r1.$1;\n\
         }\n",
    );
    let found = member_rows(&report);
    assert_eq!(found.len(), 1, "{:#?}", rows(&report));
    assert!(
        found[0].starts_with("unchecked_use_of_nullable_value "),
        "{found:?}"
    );
}

/// `?.` unwraps the receiver, so the nullable alias already reads the field
/// (oracle silent) — the corroboration that the alias folds to a record: the
/// null-aware flag alone made the nullable shape acceptable to the field
/// consult.
#[test]
fn null_aware_alias_record_reads_field() {
    let report = report_ops(
        "typedef R1 = (int, String)?;\n\
         void f(R1 r1) {\n\
           r1?.$1;\n\
         }\n",
    );
    assert!(member_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// The control: a non-nullable alias reaches the same field arm with no
/// promotion in play (oracle silent) — the fix must not be the only path
/// that resolves record fields.
#[test]
fn non_null_alias_record_reads_positional_fields() {
    let report = report_ops(
        "typedef R1 = (int, String);\n\
         void f(R1 r1) {\n\
           r1.$1;\n\
           r1.$2;\n\
         }\n",
    );
    assert!(member_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// A promoted receiver still reports an ABSENT field: the non-null face must
/// not turn a genuine miss into a silent access (oracle: one
/// `UNDEFINED_GETTER`, "The getter '$3' isn't defined for the type 'R1'.").
#[test]
fn guarded_alias_record_reports_absent_field() {
    let report = report_ops(
        "typedef R1 = (int, String)?;\n\
         void f(R1 r1) {\n\
           if (r1 != null) {\n\
             r1.$3;\n\
           }\n\
         }\n",
    );
    let found = member_rows(&report);
    assert_eq!(found.len(), 1, "{:#?}", rows(&report));
    assert!(found[0].starts_with("undefined_getter "), "{found:?}");
}

/// A promoted receiver still reports the record's missing SETTER: records are
/// immutable upstream, so a field hit resolves the read face only and the
/// write reports `UNDEFINED_SETTER` (oracle: one row, "The setter '$1' isn't
/// defined for the type 'R1'.").
#[test]
fn guarded_alias_record_write_reports_missing_setter() {
    let report = report_ops(
        "typedef R1 = (int, String)?;\n\
         void f(R1 r1) {\n\
           if (r1 != null) {\n\
             r1.$1 = 0;\n\
           }\n\
         }\n",
    );
    let found = member_rows(&report);
    assert_eq!(found.len(), 1, "{:#?}", rows(&report));
    assert!(found[0].starts_with("undefined_setter "), "{found:?}");
}
