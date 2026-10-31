//! The #3087 pins — a null-aware access whose receiver spells an invalid
//! type bound classifies null-or-equivalent, so the `?.` right side is dead
//! exactly where the oracle says it is.
//!
//! `T?` over an invalid bound is `Null`-equivalent: `T? <: Null` iff
//! `T <: Null` iff `B <: Null`, and `InvalidType <: X` answers true for
//! every `X` (upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/
//! subtype.dart:274), so `classifyType` lands on `nullOrEquivalent`
//! (`flow_analysis_visitor.dart:615-632`) and the null-shorted access never
//! runs. The drift `database.dart:92` shape is the field twin: `DB?
//! _database` over `DB extends CommonDatabase` with the bound's head
//! unresolved (`package:sqlite3/common.dart` outside the analyzed world),
//! read through implicit `this`.
//!
//! Two seams carry the verdict. Both were already on main when this ticket
//! was dispatched; the pins below verify them and lock the face:
//!
//! - the semantic staging of an implicit-`this` member read's declared face
//!   (`listener.rs` `handle_identifier` → `stage_this_member_type`, the
//!   `DB? _database` arm — upstream Dart SDK source
//!   pkg/analyzer/lib/src/dart/resolver/`ResolverVisitor`'s `this`-member
//!   read; pinned here by the field twin), which is what hands the flow
//!   layer `DB?` at the read's offset, and
//! - `Interp::classify`'s invalid-bound arm (a nullable `Base::Name` whose
//!   boundary bound answers `TypeOps::is_invalid_typed` →
//!   `NullOrEquivalent`), which reads the bound through
//!   `ResolvedTypeOps::is_invalid_typed` (`resolved_ops.rs`,
//!   `map_scrutinee` → `TypeKind::Invalid`).
//!
//! Oracle provenance: `dart analyze` 3.13.1 machine format. Each `?.`
//! fixture below reports `UNDEFINED_CLASS` at the bound head and `DEAD_CODE`
//! at the null-shorted right side; the resolved-bound control reports
//! nothing.

use super::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};
use crate::diagnose::FileReport;

/// The `dead_code` rows of `report` as `(offset, length)` pairs.
fn dead_code_spans(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|diag| diag.code.name == "dead_code")
        .map(|diag| (diag.range.offset, diag.range.length))
        .collect()
}

/// The text one reported range covers, sliced out of the mounted unit
/// (`CORE_PRELUDE` + fixture).
fn slice(source: &str, offset: u32, length: u32) -> String {
    let full = format!("{CORE_PRELUDE}{source}");
    full.get(offset as usize..(offset + length) as usize)
        .unwrap_or_default()
        .to_string()
}

/// The drift field shape (`_database?.close()` over `DB?` with an invalid
/// bound): the invocation is dead (oracle: `DEAD_CODE` `10:18` len 7 —
/// `close()`).
#[test]
fn invalid_bound_nullable_field_access_is_dead() {
    let source = "abstract class Sqlite3Delegate<DB extends CommonDatabase> {\n\
                  \x20 DB? _database;\n\
                  \n\
                  \x20 void _initializeDatabase() {}\n\
                  \n\
                  \x20 void open() {\n\
                  \x20   try {\n\
                  \x20     _initializeDatabase();\n\
                  \x20   } catch (e) {\n\
                  \x20     _database?.close();\n\
                  \x20     _database = null;\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        code_names(&report),
        vec!["undefined_class", "dead_code"],
        "{:?}",
        rows(&report)
    );
    let spans = dead_code_spans(&report);
    assert_eq!(spans.len(), 1, "exactly one dead row: {:?}", rows(&report));
    assert_eq!(
        slice(source, spans[0].0, spans[0].1),
        "close()",
        "the row spans the null-shorted invocation: {:?}",
        rows(&report)
    );
}

/// The async twin (drift's `open()` is `async`): the same dead invocation
/// (oracle: `DEAD_CODE` `10:18` len 7 — the body modifier changes nothing).
#[test]
fn invalid_bound_nullable_field_access_is_dead_in_async_body() {
    let source = "abstract class Sqlite3Delegate<DB extends CommonDatabase> {\n\
                  \x20 DB? _database;\n\
                  \n\
                  \x20 void _initializeDatabase() {}\n\
                  \n\
                  \x20 Future<void> open() async {\n\
                  \x20   try {\n\
                  \x20     _initializeDatabase();\n\
                  \x20   } catch (e) {\n\
                  \x20     _database?.close();\n\
                  \x20     _database = null;\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        code_names(&report),
        vec!["undefined_class", "dead_code"],
        "{:?}",
        rows(&report)
    );
    let spans = dead_code_spans(&report);
    assert_eq!(spans.len(), 1, "exactly one dead row: {:?}", rows(&report));
    assert_eq!(
        slice(source, spans[0].0, spans[0].1),
        "close()",
        "{:?}",
        rows(&report)
    );
}

/// The parameter twin: `value?.size` over `T?` with an invalid bound is a
/// dead member read (oracle: `DEAD_CODE` at the `size` token, len 4).
#[test]
fn invalid_bound_nullable_parameter_access_is_dead() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  void f<T extends Missing>(T? value) {\n\
                  \x20 value?.size;\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        code_names(&report),
        vec!["undefined_class", "dead_code"],
        "{:?}",
        rows(&report)
    );
    let spans = dead_code_spans(&report);
    assert_eq!(spans.len(), 1, "exactly one dead row: {:?}", rows(&report));
    assert_eq!(
        slice(source, spans[0].0, spans[0].1),
        "size",
        "{:?}",
        rows(&report)
    );
}

/// The alias-to-invalid bound (`typedef CB = Missing;` then `T extends CB`):
/// the alias resolves to `InvalidType`, so `T?` is null-equivalent and the
/// access is dead (oracle: `DEAD_CODE` at the `size` token).
#[test]
fn alias_to_invalid_bound_is_null_equivalent() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  typedef CB = Missing;\n\
                  void f<T extends CB>(T? value) {\n\
                  \x20 value?.size;\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        code_names(&report),
        vec!["undefined_class", "dead_code"],
        "{:?}",
        rows(&report)
    );
}

/// The argument-bearing invalid head (`T extends Missing<int>`): the bound
/// still resolves nowhere, so the access is dead (oracle: `DEAD_CODE`).
#[test]
fn argument_bearing_invalid_bound_is_null_equivalent() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  void h<T extends Missing<int>>(T? value) {\n\
                  \x20 value?.size;\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        code_names(&report),
        vec!["undefined_class", "dead_code"],
        "{:?}",
        rows(&report)
    );
}

/// The member-function type parameter (`void m<U extends AlsoMissing>(U? u)`
/// inside a class): the same deadness inside a member body (oracle:
/// `DEAD_CODE` at the `size` token).
#[test]
fn member_type_parameter_invalid_bound_is_null_equivalent() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  class Outer {\n\
                  \x20 void m<U extends AlsoMissing>(U? u) {\n\
                  \x20   u?.size;\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        code_names(&report),
        vec!["undefined_class", "dead_code"],
        "{:?}",
        rows(&report)
    );
}

/// The guard twin: `value != null` over the same receiver is never true, so
/// the then-block is dead (oracle: `DEAD_CODE` spanning the block).
///
/// Only the dead block is pinned. The oracle stays silent inside it, while
/// this engine still runs the receiver-face check and reports
/// `unchecked_use_of_nullable_value` on the dead `value.size` — suppressing
/// rows inside unreachable regions is the report pipeline's
/// (`row_resolution.rs` / `engine_core.rs`), not this face's.
#[test]
fn invalid_bound_nullable_guard_then_block_is_dead() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  void g<T extends Missing>(T? value) {\n\
                  \x20 if (value != null) {\n\
                  \x20   value.size;\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    assert!(
        code_names(&report).contains(&"dead_code"),
        "the guarded block is dead: {:?}",
        rows(&report)
    );
    assert_eq!(
        dead_code_spans(&report).len(),
        1,
        "exactly one dead row: {:?}",
        rows(&report)
    );
}

/// The resolvable-bound control: a `T?` whose bound IS a real class stays
/// potentially nullable, so neither the `?.` access nor the guarded block
/// is dead (oracle: no rows at all).
#[test]
fn resolvable_bound_nullable_access_stays_live() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  void f<T extends Box>(T? value) {\n\
                  \x20 value?.size;\n\
                  \x20 if (value != null) {\n\
                  \x20   value.size;\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    assert!(
        code_names(&report).is_empty(),
        "a resolved bound keeps the access live: {:?}",
        rows(&report)
    );
}
