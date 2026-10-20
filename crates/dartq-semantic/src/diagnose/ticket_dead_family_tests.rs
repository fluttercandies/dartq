
//! The dead/unused-family differential pins: the corpus `dart analyze`
//! `(severity, code, file, line:col)` burn-down, rooted in four oracle
//! mechanisms each fixture reproduces end-to-end.
//!
//! - `x is T` / `x is! T` with an unresolvable `T` (the broken-import
//!   corpus's dominant `dead_code` root): the oracle's `InvalidType` is
//!   a vacuous subtype in both directions (subtype.dart:274-276), so
//!   `factor(previous, InvalidType)` is `Never` and the losing branch
//!   reports `dead_code` at its first node — `is` deads the else,
//!   `is!` deads the then (verified against `dart analyze` 3.13.1,
//!   `sdk: ^3.13.0`).
//! - `unused_field`'s `unresolvedReadMembers` fallback
//!   (unused_local_elements_verifier.dart:1030-1055): a read-shaped
//!   member access that bound no element (`f._x` on an
//!   invalid-typed receiver) still spares the field by spelling.
//! - `_recordEnumValuesUse` (:442-455): reading an enum's `values`
//!   getter credits every constant's getter at once.
//! - The extension credit's receiver-validity gate: an access through an
//!   unresolvable receiver type (the broken-import corpus's dominant
//!   shape — `m.g` on an undefined `Missing`) binds nothing and credits
//!   nothing, so the accessed member reports; an access through a VALID
//!   receiver applies an invalid-`on` extension vacuously (the subtype
//!   check against the InvalidType extended type passes) and spares it.
//!   The credit is judged at the resolution side and the declaration
//!   walk trusts it as recorded.

use super::*;
use crate::collector::collect;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    (scanned, features, set)
}

/// Diagnoses `source` as the sole file of a library with the resolved
/// `TypeOps` face mounted — the engine path the `x is Missing` fold
/// needs (the name-level ops cannot see `InvalidType`).
fn report_ops(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
    let ops = ResolvedTypeOps::new(&types, &strings);
    let fold = fold_unit_with_ops(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// The single-file rig without a resolved ops face — the unused-*
/// verdicts are name-level by construction.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

// —— dead_code: the InvalidType checked type ————————————————————————

/// `x is Missing` deads the else branch: `factor(int, InvalidType)`
/// is `Never` — the oracle's 0c subtype arm makes `int <:
/// InvalidType` vacuously true, so the "not Missing" remainder is
/// empty and the else reports `dead_code` at its first node.
/// Oracle-verified (dart analyze 3.13.1, `sdk: ^3.13.0`): the probe
/// pairs `type_test_with_undefined_name` on the spelling with
/// `dead_code` at the else arm.
#[test]
fn is_test_with_undefined_name_deads_the_else() {
    let source = "void f(int x) {\n\
                      \x20 if (x is Missing) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let else_start = source.find("else").unwrap() as u32;
    assert!(
        dead.iter().any(|&(offset, _)| offset >= else_start),
        "the else arm of `x is Missing` is dead: {:?}",
        report.diagnostics
    );
}

/// `x is! Missing` deads the then branch — the `is`-false arm is the
/// impossible remainder, and `is!` swaps the branches
/// (`expressionInfo._invert()`).
#[test]
fn is_not_test_with_undefined_name_deads_the_then() {
    let source = "void f(int x) {\n\
                      \x20 if (x is! Missing) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    let test_start = source.find("is!").unwrap() as u32;
    let else_start = source.find("else").unwrap() as u32;
    assert!(
        dead.iter()
            .any(|&(offset, _)| offset > test_start && offset < else_start),
        "the then arm of `x is! Missing` is dead: {:?}",
        report.diagnostics
    );
}

/// A resolvable narrowing test keeps both arms live — `factor(Parent,
/// Child)` is `Parent`, not `Never`, so the else stays reachable (the
/// `Invalid` verdict must not leak into the resolved-name case; an
/// already-satisfied test deads the else in the oracle too — the
/// `unnecessary_type_check` pair — so the control narrows instead).
#[test]
fn is_test_with_resolved_name_keeps_both_arms() {
    let source = "class Parent {}\n\
                      class Child extends Parent {}\n\
                      void f(Parent x) {\n\
                      \x20 if (x is Child) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "no dead arm for a narrowing is-test: {:?}",
        report.diagnostics
    );
}

// —— unused_field: unresolvedReadMembers + enum `values` ————————————

/// `f._x` on an invalid-typed receiver spares `_x` by spelling: the
/// read bound no element, so the oracle files it under
/// `unresolvedReadMembers` — the name alone suffices for the field
/// verdict (`_isReadMember`'s second arm,
/// unused_local_elements_verifier.dart:1049-1050).
#[test]
fn unresolved_receiver_read_spares_the_field() {
    let source = "class C {\n\
                      \x20 int _x = 0;\n\
                      }\n\
                      void f(Missing m) {\n\
                      \x20 print(m._x);\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_field").is_empty(),
        "the unresolved read spares `_x`: {:?}",
        report.diagnostics
    );
}

/// The oracle's `_recordEnumValuesUse`: reading an enum's `values`
/// getter credits every constant — `E.values` on a private enum
/// leaves no `unused_field` rows even when no constant is read by
/// name.
#[test]
fn enum_values_read_spares_every_constant() {
    let source = "enum _E { a, b, c }\n\
                      void f() {\n\
                      \x20 for (var v in _E.values) {\n\
                      \x20   v;\n\
                      \x20 }\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_field").is_empty(),
        "`_E.values` spares every constant: {:?}",
        report.diagnostics
    );
}

/// A private enum constant never read by name or through `values`
/// still reports — the credit is scoped to the `.values` access, not
/// the enum's whole face.
#[test]
fn unread_enum_constant_still_reports() {
    let source = "enum _E { a, b }\n\
                      void f() {\n\
                      \x20 _E.a;\n\
                      }";
    let report = report(source);
    let bare_b = source.find("b }").unwrap() as u32;
    assert!(
        rows(&report, "unused_field")
            .iter()
            .any(|&(offset, _)| offset == bare_b),
        "`b` was never read: {:?}",
        report.diagnostics
    );
}

// —— unused_element: the extension credit's receiver-validity gate ————
//
// The oracle truth table (probed against `dart analyze` 3.13.1, a
// self-package `package_config.json` mapping the probing directory —
// the corpus's broken-import world):
//
// - an access through a VALID receiver spares a member of an
//   extension whose `on` type is itself invalid — the subtype check
//   against the InvalidType extended type passes vacuously, the member
//   lookup inside the extension succeeds, and `usedElements.members`
//   gains it (`c.g` on a valid `C` with `extension _E on Missing`
//   emits neither `undefined_getter` nor `unused_element`; dropping
//   the extension brings the getter miss back);
// - an access through an INVALID receiver (or no access at all)
//   credits nothing — the lookup dies on the unresolvable receiver
//   before the extension probe — so the member reports;
// - a bare identifier read inside a SIBLING extension is an
//   implicit-`this` read on the sibling's `on` type: no extension
//   override is created and the member stays uncredited.
//
// The credit set is therefore judged at the resolution side (receiver
// validity), and the declaration-site walk trusts it as recorded.

/// The corpus's dominant `unused_element` family: an invalid receiver
/// (`m` typed by an undefined class) credits nothing, so an accessed
/// member of an extension whose `on` type also fails to resolve still
/// reports.
#[test]
fn extension_member_on_invalid_receiver_still_reports() {
    let source = "extension _E on Missing {\n\
                      \x20 int get g => 0;\n\
                      }\n\
                      void f(Missing m) {\n\
                      \x20 m.g;\n\
                      }";
    let report = report_ops(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get g").unwrap() as u32 + 4 }),
        "the member accessed through an invalid receiver reports: {:?}",
        report.diagnostics
    );
}

/// Without any access the member reports regardless of the `on`
/// clause — the unaccessed half of the corpus family.
#[test]
fn extension_member_reports_without_access() {
    let source = "class C {}\n\
                      extension _E on Missing {\n\
                      \x20 int get g => 0;\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get g").unwrap() as u32 + 4 }),
        "the unaccessed member of `extension _E on Missing` reports: {:?}",
        report.diagnostics
    );
}

/// A public member of an UNNAMED extension is checked like a private
/// declaration (the extension itself has no name to be exported by),
/// so the corpus's `extension on Element?` getter family reports when
/// nothing applies to it.
#[test]
fn unnamed_extension_member_reports_when_never_applied() {
    let source = "extension on Missing {\n\
                      \x20 bool get isFoo => true;\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get isFoo").unwrap() as u32 + 4 }),
        "the unnamed extension's member reports: {:?}",
        report.diagnostics
    );
}

/// The ffi_verifier shape end to end: the extension's `on` type and
/// the accessing receiver's declared type are BOTH undefined, so the
/// access binds nothing and the member reports.
#[test]
fn unnamed_extension_member_reports_for_invalid_receiver_access() {
    let source = "void f(Missing m) {\n\
                      \x20 m.isFoo;\n\
                      }\n\
                      extension on Missing {\n\
                      \x20 bool get isFoo => true;\n\
                      }";
    let report = report_ops(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get isFoo").unwrap() as u32 + 4 }),
        "the member accessed through the invalid receiver reports: {:?}",
        report.diagnostics
    );
}

/// `assert(isArray)` inside a sibling extension is an implicit-`this`
/// read on the SIBLING's `on` type: no extension override engages, the
/// read credits nothing, and both the read member and the reading
/// method report.
#[test]
fn unnamed_extension_bare_sibling_read_credits_nothing() {
    let source = "extension on A {\n\
                      \x20 bool get isArray => true;\n\
                      }\n\
                      extension on B {\n\
                      \x20 void m() {\n\
                      \x20\x20 assert(isArray);\n\
                      \x20 }\n\
                      }\n\
                      class A {}\n\
                      class B {}";
    let report = report(source);
    let getter_at = source.find("get isArray").unwrap() as u32 + 4;
    let method_at = source.find("void m").unwrap() as u32 + 5;
    let unused = rows(&report, "unused_element");
    assert!(
        unused.iter().any(|&(offset, _)| offset == getter_at),
        "the cross-extension bare read spares nothing: {:?}",
        report.diagnostics
    );
    assert!(
        unused.iter().any(|&(offset, _)| offset == method_at),
        "the never-applied sibling method reports: {:?}",
        report.diagnostics
    );
}

/// The `visitWithOverride` corpus shape: a generic unnamed extension
/// (`extension<T extends C> on T`) whose parameter-headed `on` type
/// can never resolve — its method reports when never applied.
#[test]
fn unnamed_generic_extension_method_reports() {
    let source = "class C {}\n\
                      extension<T extends C> on T {\n\
                      \x20 void visitWithOverride() {}\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("visitWithOverride").unwrap() as u32 }),
        "the generic unnamed extension's method reports: {:?}",
        report.diagnostics
    );
}

/// A named-private extension over a resolving generic `on` head
/// (`Map<K, V>` with local arguments) that is never applied: the
/// member reports despite the resolvable head.
#[test]
fn named_private_extension_generic_on_reports_when_never_applied() {
    let source = "class K {}\n\
                      class V {}\n\
                      extension _M on Map<K, V> {\n\
                      \x20 int get entry => 1;\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element")
            .iter()
            .any(|&(offset, _)| { offset == source.find("get entry").unwrap() as u32 + 4 }),
        "the never-applied member of the generic-`on` extension reports: {:?}",
        report.diagnostics
    );
}

/// The resolved `on` control: `extension _E on C` applies to `c.g`,
/// so the member spares.
#[test]
fn extension_on_resolved_type_credits_members() {
    let source = "class C {}\n\
                      extension _E on C {\n\
                      \x20 int get g => 0;\n\
                      }\n\
                      void f(C c) {\n\
                      \x20 c.g;\n\
                      }";
    let report = report(source);
    assert!(
        rows(&report, "unused_element").is_empty(),
        "the member of `extension _E on C` spares: {:?}",
        report.diagnostics
    );
}
