//! The #2420 pins: the supertype member walk survives the dead supertype
//! hop (LAW B), and a broken chain over a names-only SDK face answers
//! through the frozen member tables (D1).
//!
//! LAW B (`analyzer-donly-retriage` on the analyzer corpus): a dead-edge
//! supertype hop — undefined, ambiguous, or non-class spelling, the oracle's
//! `InvalidType` — contributes nothing to the interface, but it no longer
//! ends the climb. The `implements` faces collected along the climb still
//! merge exactly like the `superclass: None` arm's tail, so
//! `class AstNodeImpl extends SyntacticEntity implements AstNode` (where
//! `SyntacticEntity` is an unresolvable `_fe_analyzer_shared` hop) still
//! resolves `accept`/`endToken`/`token` against `AstNode`. Pre-fix every
//! such member read emitted `undefined_*` (~1166 dartq-only rows:
//! `UNDEFINED_METHOD`/`UNDEFINED_GETTER`/`UNDEFINED_IDENTIFIER`/
//! `OVERRIDE_ON_NON_OVERRIDING_MEMBER`/`INVALID_IMPLEMENTATION_OVERRIDE`/
//! `UNDEFINED_SETTER`). Post-fix the walk still reports the oracle's own
//! rows — a dead `extends` with no interfaces, or a member the live
//! `implements` face does not carry, is the clean `Walk::Missing`.
//!
//! D1 (`abstract class S extends Iterable<MissingItem>`): the oracle keeps
//! a supertype whose *name* resolves even when a type argument is the
//! error type — `iterator`/`first`/`length`/`whereType` read silently and
//! only `non_type_as_type_argument` reports. The collector already keeps
//! the superclass name; the element index just cannot model the synthetic
//! `<dart:core>` stub, so the climb used to answer `ChainBroken` and the
//! bare-member arm reported `undefined_*` for every inherited name. Now
//! the member arm refines a broken chain through the frozen member tables
//! (the same three-valued verdict the dotted-member arm consults):
//! `Defined` claims the member, `Absent`/`Unknown` keep the fall-through —
//! so `iterableAbsent` still reports `undefined_method` while `iterator`
//! stays silent, whether the argument resolves or not.
//!
//! Oracle provenance: probed against `dart analyze` (Dart 3.13.1, fvm
//! 3.47.1) under `/tmp/probe2272` — `micro/h-iterable-generic`,
//! `repro/r4a`-`r4c`, `micro/dead-impl`, `micro/dead-iface-miss`,
//! `micro/implements-dead-implements`, `micro/nonclass`,
//! `micro/mixin-dead`, `micro/frozen-ext`, `micro/frozen-miss`,
//! `micro/frozen-getter`, `micro/ext-list`.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `main_source` as `main.dart` importing `lib.dart`, a two-file
/// library — `AstNodeImpl`/`AstNode`/`SyntacticEntity` live in `lib.dart`
/// (the external-library supertype hop) while the member reads run in
/// `main.dart` (the `extends DeadSyntacticEntity implements AstNode`
/// hop plus the bare/dotted consumers).
fn report_two_file(main_source: &str, lib_source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_lib = files.intern(Path::new("lib.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let scan = |source: &str, file: FileId| {
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
        scanned
    };
    let scanned_main = scan(main_source, file_main);
    let scanned_lib = scan(lib_source, file_lib);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_main = collect(main_source, &scanned_main.tokens, features, &strings);
    let elements_lib = collect(lib_source, &scanned_lib.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let lib_lib = builder.intern("package:t/lib.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements_main.clone());
    index.insert(file_lib, elements_lib.clone());
    for (name, decl) in elements_main.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_main, name, file_main);
    }
    for (name, decl) in elements_lib.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_lib, name, file_lib);
    }
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.seed_implicit_core(
        lib_main,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "Iterable", "void",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        main_source,
        &scanned_main.tokens,
        features,
        &strings,
        file_main,
        lib_main,
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

/// Single-file rig for the dead-hop shapes that stay inside one unit
/// (`extends Live`/`implements DeadFace` and the `extends Iterable<…>`
/// probe): `report_one` runs the same `package:t/main.dart` seed plus the
/// `dart:core` imports the bare `Iterable` spelling needs.
fn report(source: &str) -> FileReport {
    report_two_file(source, "")
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

fn has(names: &[&'static str], code: &str) -> bool {
    names.contains(&code)
}

/// LAW B: a dead superclass edge with a live `implements` face merges the
/// interface's members — `accept`/`endToken`/`token` (dotted and bare) and
/// `liveAbsent` (the miss) mirror the oracle's result on the analyzer
/// corpus's `AstNodeImpl extends SyntacticEntity implements AstNode` (the
/// `SyntacticEntity` spelling lives in `package:_fe_analyzer_shared`, which
/// the corpus's `package_config` leaves unresolved).
#[test]
fn dead_superclass_implements_face_still_walks() {
    let lib_source = "abstract class AstNode {\n\
                      \x20 int accept(int visitor);\n\
                      \x20 int get endToken;\n\
                      }\n\
                      class DeadSyntacticEntity {}\n";
    let main_source = "class Impl extends DeadSyntacticEntity implements AstNode {\n\
                       \x20 int accept(int visitor) => visitor;\n\
                       \x20 int get endToken => 0;\n\
                       \x20 int get token => 0;\n\
                       }\n\
                       void f(Impl node) {\n\
                       \x20 node.accept(1);\n\
                       \x20 node.endToken;\n\
                       \x20 node.token;\n\
                       }\n";
    let names = code_names(&report_two_file(main_source, lib_source));
    assert!(
        !has(&names, "undefined_method"),
        "accept resolves through the AstNode face: {names:?}"
    );
    assert!(
        !has(&names, "undefined_getter"),
        "endToken/token resolve through the AstNode face: {names:?}"
    );
    assert!(
        !has(&names, "undefined_identifier"),
        "no bare-name miss once the face is reachable: {names:?}"
    );
}

/// LAW B — the live-face miss still reports: `absent` names nothing on the
/// `AstNode` interface, so the dead superclass edge is a clean miss once the
/// interface merge runs.
#[test]
fn dead_superclass_implements_miss_still_reports() {
    let lib_source = "abstract class AstNode {\n\
                      \x20 int accept(int visitor);\n\
                      }\n\
                      class DeadSyntacticEntity {}\n";
    let main_source = "class Impl extends DeadSyntacticEntity implements AstNode {\n\
                       \x20 int accept(int visitor) => visitor;\n\
                       }\n\
                       void f(Impl node) {\n\
                       \x20 node.absent;\n\
                       }\n";
    let names = code_names(&report_two_file(main_source, lib_source));
    assert!(has(&names, "undefined_getter"), "{names:?}");
}

/// A dead `extends` with no `implements` faces is the clean miss the
/// member lookups report (the `superclass: None` tail with an empty
/// pending list — `dead-iface-miss`'s oracle row).
#[test]
fn dead_superclass_alone_reports() {
    let source = "class Impl extends DeadSyntacticEntity {\n\
                  \x20 void f() {\n\
                  \x20   this.entityAbsent();\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "undefined_method"), "{names:?}");
}

/// A dead `implements` seed face contributes nothing and reports
/// (`extends Object implements DeadFace` with no live interfaces — the
/// `implements-dead-implements` probe's `absentMember` row).
#[test]
fn dead_implements_face_reports() {
    let source = "class Impl extends Object implements DeadFace {\n\
                  \x20 void f() {\n\
                  \x20   this.absentMember();\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "undefined_method"), "{names:?}");
}

/// A non-class supertype spelling is the same dead edge — `extends topFn`
/// reports its own miss (`nonclass` probe's `fnAbsent` row).
#[test]
fn nonclass_supertype_reports() {
    let source = "void topFn() {}\n\
                  class Impl extends topFn {\n\
                  \x20 void f() {\n\
                  \x20   this.fnAbsent();\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "undefined_method"), "{names:?}");
}

/// A dead `with` mixin contributes nothing but the chain keeps climbing —
/// `extends Live with DeadMixin` still resolves `liveMember` on the live
/// superclass and reports `absentMember` (`mixin-dead` probe).
#[test]
fn dead_mixin_hop_keeps_the_chain() {
    let source = "class Live {\n\
                  \x20 void liveMember() {}\n\
                  }\n\
                  class Impl extends Live with DeadMixin {\n\
                  \x20 void f() {\n\
                  \x20   this.liveMember();\n\
                  \x20   this.absentMember();\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert_eq!(
        names.iter().filter(|n| **n == "undefined_method").count(),
        1,
        "exactly one undefined_method — liveMember resolves through Live and only absentMember reports: {names:?}"
    );
}

/// D1: `extends Iterable<MissingItem>` keeps the supertype — the error type
/// argument reports once (`non_type_as_type_argument`) and every `Iterable`
/// member reads silently. The names-only `<dart:core>` stub carries
/// `Iterable` as a frozen face; a `ChainBroken` walk over it now answers the
/// declared member surface instead of the under-report.
#[test]
fn iterable_error_arg_keeps_the_supertype() {
    let source = "abstract class S extends Iterable<MissingItem> {\n\
                  \x20 void f() {\n\
                  \x20   iterator;\n\
                  \x20   first;\n\
                  \x20   length;\n\
                  \x20   whereType<S>();\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        has(&names, "non_type_as_type_argument"),
        "the error type argument still reports: {names:?}"
    );
    assert!(
        !has(&names, "undefined_method"),
        "whereType stays silent: {names:?}"
    );
    assert!(
        !has(&names, "undefined_getter") && !has(&names, "undefined_identifier"),
        "iterator/first/length stay silent: {names:?}"
    );
}

/// D1 — the same shape with a resolved argument is silent too: the fix is
/// the resolved-but-unindexed head, not the error arg itself
/// (`frozen-miss` probe: `extends Iterable<int>`).
#[test]
fn iterable_resolved_arg_stays_silent() {
    let source = "abstract class S extends Iterable<int> {\n\
                  \x20 void f() {\n\
                  \x20   iterator;\n\
                  \x20   first;\n\
                  \x20   length;\n\
                  \x20   whereType<S>();\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        !has(&names, "undefined_method")
            && !has(&names, "undefined_getter")
            && !has(&names, "undefined_identifier"),
        "{names:?}"
    );
}

/// A genuinely-missing member on the frozen `Iterable` face still reports —
/// `iterableAbsent` names nothing `Iterable` declares (`frozen-ext`'s
/// oracle row: `undefined_method` of `S`, beside the argument's
/// `non_type_as_type_argument`).
#[test]
fn iterable_missing_member_reports() {
    let source = "abstract class S extends Iterable<MissingItem> {\n\
                  \x20 void f() {\n\
                  \x20   iterableAbsent();\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "non_type_as_type_argument"), "{names:?}");
    assert!(
        has(&names, "undefined_method"),
        "the frozen face's absent member still reports: {names:?}"
    );
}

/// `@override` on a member the interface does not carry still reports the
/// `override_on_non_overriding_member` family — the genuinely-missing-override
/// case the oracle emits (`dead-impl`'s `liveAbsent` row).
#[test]
fn override_on_missing_member_reports() {
    let source = "class Impl extends Object implements Live {\n\
                  \x20 @override\n\
                  \x20 void liveAbsent() {}\n\
                  }\n\
                  abstract class Live {\n\
                  \x20 void present();\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        has(&names, "override_on_non_overriding_member"),
        "{names:?}"
    );
}

/// The cross-file face anchor keeps working after the hop fix: a superclass
/// living in `lib.dart` resolves its members in `main.dart` (the plain
/// `extends LiveLib` row — the `pending_faces` merge must not steal it).
#[test]
fn cross_file_superclass_still_resolves() {
    let lib_source = "class LiveLib {\n\
                      \x20 int liveMember() => 0;\n\
                      }\n";
    let main_source = "class Impl extends LiveLib {\n\
                       \x20 void f() {\n\
                       \x20   this.liveMember();\n\
                       \x20 }\n\
                       }\n";
    let names = code_names(&report_two_file(main_source, lib_source));
    assert!(!has(&names, "undefined_method"), "{names:?}");
}
