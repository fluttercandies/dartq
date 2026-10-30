//! The #2928 small-faces pins: the typedef wildcard drain, the top-level
//! setter `f=` duplicate spelling, and the `in T` variance argument
//! relation.
//!
//! 1. The wildcard-`_` leftovers drain (#2914 residual 1): a pure
//!    typedef's `typedef F<_, _>` type-parameter duplicate parks in the
//!    wildcard log — `end_typedef` fires no executable-end drain hook —
//!    so the fold's duplicate assembly renders the not-yet-drained
//!    suffix (the same cursor-gated posture as the walk-time
//!    [`Engine::emit_wildcard_dups`]; upstream `isWildcardVariable`,
//!    duplicate_definition_verifier.dart:350-360).
//! 2. The duplicate message spelling (#2914 residual 4): a top-level
//!    setter duplicates under its `name=` lookup spelling and the
//!    message renders it verbatim (`checkUnit`'s `definedSetters`,
//!    duplicate_definition_verifier.dart:116-307 — the census rows
//!    `DuplicateDefinitionTest/test_topLevel_setter_setter#0` and
//!    `test_topLevel_field_setter#0` read `The name 'f=' is already
//!    defined.` byte-exact), while the class-member face captures the
//!    bare lexeme and stays bare.
//! 3. The variance argument relation (subtype.dart:44-83
//!    `_interfaceArguments`): a declared `in` parameter asks its slot
//!    right-to-left (`C<A>` <: `C<B>` iff `B <: A`), an `inout`
//!    parameter both ways, a legacy head keeps the covariant
//!    left-to-right ask — the census faces
//!    `variance_in_subtyping_test`/`_error_test` ride this flip.

use super::test_rig::{code_names, report_ops, report_ops_wildcards, rows};
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, fold_unit, merge_source_ordered};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

// —— face 1: the typedef wildcard drain ——

/// A pure typedef's `_, _` type-parameter duplicate reports once the
/// fold's leftovers drain runs — pre-feature, the second `_` is an
/// ordinary duplicate name.
#[test]
fn typedef_wildcard_drains_pre_feature() {
    let report = report_ops_wildcards("typedef F<_, _> = int;\n", /* wildcards */ false);
    let dups: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(
        dups,
        vec!["The name '_' is already defined.".to_string()],
        "the parked typedef wildcard drains at the fold: {dups:?}"
    );
}

/// Under the wildcard-variables feature the `_` declares no name — the
/// log stays parked and the fold drain emits nothing.
#[test]
fn typedef_wildcard_stays_silent_under_feature() {
    let report = report_ops_wildcards("typedef F<_, _> = int;\n", /* wildcards */ true);
    let dups: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.message.to_string())
        .collect();
    assert!(
        dups.is_empty(),
        "feature-on `_` declares no name: {dups:?} — full rows {:?}",
        rows(&report)
    );
}

/// A unit whose class end drains the log mid-walk (the class's own `_`
/// type parameters) must not double-render through the fold's leftovers
/// drain: the cursor parks the log single-shot.
#[test]
fn drained_wildcards_never_double_render() {
    let report = report_ops_wildcards(
        "typedef F<_, _> = int;\nclass C<_, _> {}\n",
        /* wildcards */ false,
    );
    assert_eq!(
        code_names(&report),
        vec!["duplicate_definition", "duplicate_definition"],
        "two wildcard faces, one emission each: {:?}",
        rows(&report)
    );
}

// —— face 2: the setter duplicate spelling ——

/// Two top-level setters named `f`: the second reports under the `name=`
/// lookup spelling (`definedSetters`' `lookupName` render).
#[test]
fn top_level_setter_setter_reports_lookup_spelling() {
    let report = report_ops("set f(int value) {}\nset f(int value) {}\n");
    let dups: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(
        dups,
        vec!["The name 'f=' is already defined.".to_string()],
        "the oracle keeps `f=` byte-exact: {dups:?}"
    );
}

/// A non-final top-level variable induces its `f=` setter claim; the
/// written `set f` collides with it under the same lookup spelling.
#[test]
fn top_level_field_setter_reports_lookup_spelling() {
    let report = report_ops("int f = 1;\nset f(int value) {}\n");
    let dups: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(
        dups,
        vec!["The name 'f=' is already defined.".to_string()],
        "the induced-setter collision renders `f=`: {dups:?}"
    );
}

/// The class-member face captures the bare lexeme (`member_dup_claim`),
/// so its duplicate message stays bare — the verbatim render must not
/// leak the top-level `=` spelling into members.
#[test]
fn member_setter_duplicate_stays_bare() {
    let report = report_ops("class C {\n  set b(int v) {}\n  set b(int v) {}\n}\n");
    let dups: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(
        dups,
        vec!["The name 'b' is already defined.".to_string()],
        "member dups capture the bare lexeme: {dups:?}"
    );
}

// —— face 3: the variance argument relation ——

/// Diagnoses `source` with the `variance` experiment granted — the `in`/
/// `inout` keywords only parse under the grant, and the declared-variance
/// facts the argument relation reads ride the same capture.
fn report_variance(source: &str) -> FileReport {
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15))
        .with_experiment(Feature::Variance);
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
        set.is_enabled(Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn atna_messages(report: &FileReport) -> Vec<String> {
    let mut messages: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.to_string())
        .collect();
    messages.sort();
    messages
}

/// `B<Upper>()` into a `B<Middle>` formal over `class B<in T>`: the
/// contravariant parameter asks its slot right-to-left (`Middle <: Upper`
/// holds) — the argument direction accepts.
#[test]
fn contravariant_formal_accepts_argument_direction() {
    let report = report_variance(
        "class B<in T> {}\n\
         class Upper {}\n\
         class Middle extends Upper {}\n\
         void take(B<Middle> b) {}\n\
         void main() {\n\
         \x20 take(B<Upper>());\n\
         }\n",
    );
    assert!(
        atna_messages(&report).is_empty(),
        "the `in` flip accepts the superargument: {:?} — full rows {:?}",
        atna_messages(&report),
        rows(&report)
    );
}

/// The rejection boundaries: a `B<Lower>` argument into the `B<Middle>`
/// formal flips to `Middle <: Lower`, false — the row reports; an
/// `inout` parameter asks both ways and a one-way fit rejects; a legacy
/// head keeps the covariant ask (`L<Lower>` fits, `L<Upper>` reports).
#[test]
fn variance_argument_rejection_boundaries() {
    let report = report_variance(
        "class B<in T> {}\n\
         class D<inout T> {}\n\
         class L<T> {}\n\
         class Upper {}\n\
         class Middle extends Upper {}\n\
         class Lower extends Middle {}\n\
         void takeB(B<Middle> b) {}\n\
         void takeD(D<Middle> d) {}\n\
         void takeL(L<Middle> l) {}\n\
         void main() {\n\
         \x20 takeB(B<Lower>());\n\
         \x20 takeD(D<Upper>());\n\
         \x20 takeL(L<Lower>());\n\
         }\n",
    );
    assert_eq!(
        atna_messages(&report).len(),
        2,
        "the flipped `in` and both-ways `inout` reject, the legacy covariant \
         ask accepts: {:?}",
        atna_messages(&report)
    );
}
