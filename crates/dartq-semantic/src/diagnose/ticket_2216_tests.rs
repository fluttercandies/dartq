
//! The #2216-family pins: the S5 faces verified end-to-end through the
//! single-file rig.
//!
//! - `for_in_of_invalid_type`: the `_checkForEachParts` iterable face —
//!   a resolved non-sequence iterable type reports at the iterable
//!   expression (oracle 3.13.1 probes; `error_verifier.dart:4890-4958`).
//! - `invalid_implementation_override`: the `final class X extends Y`
//!   face — an inherited concrete member that loses against the
//!   interface map's most-specific signature reports at the class name
//!   (the analyzer `DirectiveUri` hierarchy shape).
//! - The #2193 role split: a missing name inside an `is`/`as` target's
//!   type-argument list reports `non_type_as_type_argument` (never the
//!   operand's dedicated cast/test code).
//! - The #2188 extension face: named private extensions enter the
//!   unused-member face; an instance member spares only on an
//!   element-attributed application.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the `tests.rs` `report` rig mirrored (the sibling rig stays off-
/// limits while its lane is in flight).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
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
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

// —— #2216 S5: for_in_of_invalid_type ———————————————————————————————

/// The collection face's minimal core stubs: `Iterable`/`List` as
/// resolvable named types (`int`/`String` are builtins) — the rig
/// carries no dart:core, so the written names declare locally to keep
/// the assignability walk's answers honest (`List<int>` really is
/// assignable to `Iterable<dynamic>` through the written edge).
const CORE_STUBS: &str = "class Iterable<T> {}\n\
                             class List<T> implements Iterable<T> {}\n\
                             class Stream<T> {}\n";

/// A resolved non-iterable type in the for-in position reports once at
/// the iterable expression (the oracle's `isAssignableTo` arm). `int`
/// is the final primitive disjoint from `Iterable` — the decidable
/// negative.
#[test]
fn for_in_reports_non_iterable_type() {
    let source = "void f(int n) { for (var x in n) {} }";
    let names = code_names(&report(source));
    assert_eq!(
        names
            .iter()
            .filter(|c| **c == "for_in_of_invalid_type")
            .count(),
        1,
        "{names:?}"
    );
}

/// A `List` source reaches `Iterable` through the written edge; a
/// nullable or dynamic source and a plain `Iterable` all stay silent
/// (the oracle's isNullable/isTop arms).
#[test]
fn for_in_stays_silent_on_iterables_and_nullable() {
    for body in [
        "void f(List<int> xs) { for (var x in xs) {} }",
        "void f(Iterable<int> xs) { for (var x in xs) {} }",
        "void f(dynamic d) { for (var x in d) {} }",
        "void f(List<int>? xs) { for (var x in xs) {} }",
    ] {
        let source = format!("{CORE_STUBS}{body}");
        let names = code_names(&report(&source));
        assert!(
            !names.contains(&"for_in_of_invalid_type"),
            "{body:?} -> {names:?}"
        );
    }
}

/// The `await` form asks `Stream<dynamic>`: a `Stream` source stays
/// silent, a resolved non-stream type reports `Stream` in the message.
#[test]
fn await_for_in_asks_stream() {
    let bad = format!("{CORE_STUBS}void f(List<int> xs) async* {{ await for (var x in xs) {{}} }}");
    let names = code_names(&report(&bad));
    assert!(names.contains(&"for_in_of_invalid_type"), "{names:?}");
    let good =
        format!("{CORE_STUBS}void f(Stream<int> xs) async* {{ await for (var x in xs) {{}} }}");
    let names = code_names(&report(&good));
    assert!(!names.contains(&"for_in_of_invalid_type"), "{names:?}");
}

// —— #2193: the is/as type-argument role split ———————————————————————

/// `x is List<Missing>` reports `non_type_as_type_argument` at the
/// miss — never `type_test_with_undefined_name` (the oracle walks the
/// operand's argument tree through the plain context chain). `List`
/// declares locally so the operand head resolves.
#[test]
fn is_operand_arg_miss_reports_arg_code() {
    let source = "class List<T> {}\nvoid f(Object x) { if (x is List<Missing>) {} }";
    let names = code_names(&report(source));
    assert!(names.contains(&"non_type_as_type_argument"), "{names:?}");
    assert!(
        !names.contains(&"type_test_with_undefined_name"),
        "{names:?}"
    );
}

/// `x is Missing` keeps the operand code — a bare-head miss is the
/// `visitIsExpression` face.
#[test]
fn is_operand_head_miss_keeps_operand_code() {
    let source = "void f(Object x) { if (x is Missing) {} }";
    let names = code_names(&report(source));
    assert!(
        names.contains(&"type_test_with_undefined_name"),
        "{names:?}"
    );
    assert!(!names.contains(&"non_type_as_type_argument"), "{names:?}");
}

/// The `as` twin: `x as List<Missing>` reports the argument code, not
/// `cast_to_non_type`.
#[test]
fn as_operand_arg_miss_reports_arg_code() {
    let source = "class List<T> {}\nvoid f(Object x) { x as List<Missing>; }";
    let names = code_names(&report(source));
    assert!(names.contains(&"non_type_as_type_argument"), "{names:?}");
    assert!(!names.contains(&"cast_to_non_type"), "{names:?}");
}

// —— #2216 S5: invalid_implementation_override, the final-class face —

/// A concrete `final` class that overrides the inherited abstract
/// `Source get source` (from the sealed intermediate) with its own
/// `FileSource get source` is a valid implementation: the own declared
/// member seats `interface.map[name]` (upstream `_getTypeMembers`,
/// `inheritance_manager3.dart:1336-1352`), so the inherited concrete
/// implementation from `Root` never becomes the comparison target.
/// Oracle-verified on fvm 3.13.1 (#2408): zero diagnostics.
#[test]
fn final_class_inherited_concrete_loses_to_interface() {
    let source = "class Source {}\n\
                      class FileSource extends Source {}\n\
                      sealed class Root { Source? get source => null; }\n\
                      sealed class Mid extends Root { Source get source; }\n\
                      final class Leaf extends Mid { FileSource get source => FileSource(); }\n";
    let names = code_names(&report(source));
    assert!(
        !names.contains(&"invalid_implementation_override"),
        "{names:?}"
    );
    assert!(names.is_empty(), "{names:?}");
}

// —— #2217 S6: the unnamed-extension `this` type —————————————————————

/// `extension on T?` types `this` as `T?` — the unnamed-extension
/// twin of the named-face `on` read (analyzer
/// `file_state_filter.dart:186` repro: `var self = this; if (self ==
/// null)` is a real null check, never `unnecessary_null_comparison`).
/// The named and unnamed shapes both stay silent on the check and on
/// `this`-fed inference.
#[test]
fn unnamed_extension_nullable_on_keeps_null_comparison() {
    for source in [
        "class L {}\nextension on L? { void m() { var self = this; if (self == null) {} } }",
        "class L {}\nextension E on L? { void m() { var self = this; if (self == null) {} } }",
        "class L {}\nextension on L? { void m() { if (this == null) {} } }",
    ] {
        let names = code_names(&report(source));
        assert!(
            !names.contains(&"unnecessary_null_comparison"),
            "{source:?} -> {names:?}"
        );
    }
}

/// The non-nullable twin stays armed: `extension on L` keeps `this`
/// strictly non-nullable, so `this == null` still reports (the face's
/// negative is the fix's own false-positive class).
#[test]
fn unnamed_extension_non_nullable_on_reports_null_comparison() {
    let source = "class L {}\nextension on L { void m() { if (this == null) {} } }";
    let names = code_names(&report(source));
    assert!(names.contains(&"unnecessary_null_comparison"), "{names:?}");
}

// —— #2188: the extension-member usage face —————————————————————————

/// A named PRIVATE extension's unused member reports
/// `unused_element` — the named-private shape now joins the face the
/// unnamed one always had (the oracle's `_isPubliclyAccessible`: an
/// extension member is accessible iff its extension is, and a
/// private-named extension is library-visible).
#[test]
fn private_named_extension_unused_member_reports() {
    let source = "class L {}\nextension _E on L { int get unused => 0; }\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"unused_element"), "{names:?}");
}

/// An extension member credited by a resolved implicit application
/// stays silent — `l.used` applies `_E.used` on `L` (the element-
/// attributed credit), even though the extension itself is private.
#[test]
fn private_named_extension_applied_member_stays_silent() {
    let source = "class L {}\n\
                      extension _E on L { int get used => 0; int get unused => 1; }\n\
                      void f(L l) { l.used; }\n";
    let report = report(source);
    let names = code_names(&report);
    let unused = names.iter().filter(|c| **c == "unused_element").count();
    assert_eq!(unused, 1, "{names:?}");
}

// —— #2216: the broken-chain iterable face ——————————————————————————

/// A supertype chain with a dead hop (the oracle's `InvalidType`
/// supertype — `class NL extends DelegatingList` over a broken
/// import) never reaches the sequence head: the for-in iterable face
/// reports even though the name-lattice ask stays open (the
/// broken-package xml corpus's `XmlNodeList` family). The
/// `undefined_class` mentions ride alongside; the pin is the
/// `for_in_of_invalid_type` row.
#[test]
fn for_in_reports_broken_chain_iterable() {
    let source = "class NL extends DelegatingList {}\n\
                      void m(NL l) { for (final x in l) {} }\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"for_in_of_invalid_type"), "{names:?}");
}

/// The clean-chain control: a written `implements` edge to the core
/// face reaches the sequence head — the same for-in stays silent.
#[test]
fn for_in_clean_chain_stays_silent() {
    let source = "class Iterable<T> {}\n\
                      class NL implements Iterable<int> {}\n\
                      void m(NL l) { for (final x in l) {} }\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"for_in_of_invalid_type"), "{names:?}");
}

/// The `await for` form asks `Stream` with the same broken-chain
/// verdict.
#[test]
fn await_for_in_reports_broken_chain_iterable() {
    let source = "class NS extends DelegatingStream {}\n\
                      void m(NS s) async { await for (final x in s) {} }\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"for_in_of_invalid_type"), "{names:?}");
}
