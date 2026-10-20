//! The #2348 differential pins: the RETURN_OF_INVALID_TYPE over-fire
//! census (batch 5, 16 packages) classified to its per-line truth, plus
//! the P0 pool-guard panic the census triage uncovered on the same file.
//!
//! Classification of the eight d-only lines (oracle silent, dartq fired
//! at the batch-5 build): every line's CONTEXT side (the declared return)
//! was correct and the ACTUAL side carried a recovery artifact — the
//! typed map literal's nested argument collapsing under the flat
//! type-use scan (http `base_response.dart`, xml `functions/node.dart`;
//! fixed by the recursive group parse #2317②, whose face
//! `ticket_2326_tests::typed_map_nested_value_argument_stays_intact`
//! keeps locked), the record-literal field reads taking the local's
//! declared type where flow promotes (drift `sqlite3/database.dart:180`),
//! the nullable function FIELD promotion before its invocation (riverpod
//! `notifier_provider.dart:502`), the nullable LOCAL promotion after its
//! assignment (riverpod `provider_listenable_transformer2.dart:237`), and
//! the bare-identifier implicit-`call` invocation typing the callee's own
//! class instead of `call`'s return (xml `expressions/statement.dart:47`,
//! `expressions/function.dart:241`).
//!
//! The root fixes live upstream of the return gate — the flow promotion
//! interpreter and the resolution read rows — and every recovery pair is
//! IDENTICAL to an oracle-reported twin (`return _create;` the
//! unpromoted callable, `return outerSub;` before the promotion,
//! `return body;` the callee object, a genuinely unpromoted record
//! field), so a pair-level suppression at the return gate would break the
//! earned face. The pins here therefore lock the earned twins — the
//! questions the gate must keep judging — and leave the recovery faces to
//! their owning lanes.
//!
//! The P0: `covariant_args` held the pool `Ref` across its element
//! verdicts, and an instantiated-alias slot re-entered `intern_list`'s
//! `borrow_mut` — `RefCell already borrowed` at the process level
//! (probed against the pre-fix build with
//! `typedef Pred<T> = bool Function(T)` under `Box<Pred<int>>` into
//! `Box<Pred<String>>`). The narrow-borrow fix copies the slots out and
//! drops the guard before the recursion; the swap-back is non-vacuous —
//! reverting the guard scoping restores the panic on the first pin.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig, local to this module.
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

fn codes(report: &FileReport, name: &str) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| d.range.offset)
        .collect()
}

// —— P0: the covariant_args pool guard across the alias recursion ———————

/// The full chain the panic rode: the covariant ask on `Box<Pred<B>>`
/// into `Box<Pred<A>>` slots the instantiated aliases — each slot verdict
/// re-enters `assignable` → `instantiated_alias_sig` → `intern_list`. The
/// oracle reports the assignment mismatch (`Pred<B>` `<:` `Pred<A>` fails
/// the contravariant formal leg); the verdict must arrive without the
/// pool's `borrow_mut` panicking against the loop's live `Ref`.
#[test]
fn covariant_alias_argument_chain_completes_without_pool_panic() {
    let source = "class A {}\n\
                  \x20class B {}\n\
                  \x20class D {}\n\
                  \x20typedef Pred<T> = D Function(T);\n\
                  \x20void take(Box<Pred<A>> a, Box<Pred<B>> b) {\n\
                  \x20  a = b;\n\
                  \x20}";
    let report = report(source);
    let assign_at = source.find("= b").unwrap() as u32 + 2;
    assert_eq!(
        codes(&report, "invalid_assignment"),
        vec![assign_at],
        "{:?}",
        report.diagnostics
    );
}

/// The matching-argument control: the same slot walk with equal written
/// arguments decides true and stays silent.
#[test]
fn covariant_alias_matching_args_stay_silent() {
    let source = "class A {}\n\
                  \x20class D {}\n\
                  \x20typedef Pred<T> = D Function(T);\n\
                  \x20class Box<T> {\n\
                  \x20  T value;\n\
                  \x20  Box(this.value);\n\
                  \x20}\n\
                  \x20void take(Box<Pred<A>> b) {}\n\
                  \x20void main() {\n\
                  \x20  take(Box<Pred<A>>((a) => A()));\n\
                  \x20}";
    let report = report(source);
    assert!(
        codes(&report, "argument_type_not_assignable").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

// —— the earned twins the return gate must keep judging —————————————————

/// The drift `sqlite3/database.dart:180` family's earned twin: without
/// the promotion the record field genuinely carries `C?` and the return
/// reports — the pair a blanket record-return suppression would lose.
/// The promoted side of this family (the census line itself) needs the
/// flow promotion interpreter's record-field reads; the recovery pair
/// shares this exact (from, to), so no gate-level suppression is sound.
/// The drift `sqlite3/database.dart:180` family's earned twin: returning
/// the unpromoted nullable local is a genuine `C?` into `C` — reports.
/// (The literal-record spelling of this twin builds its from-row through
/// the SDK-seeded read rows the single-file rig does not seed, so the pin
/// locks the direct-return form of the identical assignability question.)
/// The promoted side of the family (the census line) needs the flow
/// promotion interpreter's record-field reads; the recovery pair shares
/// this exact (from, to), so no gate-level suppression is sound.
#[test]
fn record_return_of_unpromoted_nullable_field_reports() {
    let source = "class C {}\n\
                  \x20class H {\n\
                  \x20  C? maybe() => null;\n\
                  \x20  C get() {\n\
                  \x20    final c = maybe();\n\
                  \x20    return c;\n\
                  \x20  }\n\
                  \x20}";
    let report = report(source);
    assert_eq!(
        codes(&report, "return_of_invalid_type").len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// The riverpod `notifier_provider.dart:502` family's earned twin:
/// returning the unpromoted callable FIELD itself is a genuine
/// `T Function()?` into `T` — reports. The invoked-and-promoted side of
/// the family (the census line) needs the flow field promotion; the
/// recovery pair shares this exact (from, to).
#[test]
fn return_of_unpromoted_function_field_itself_reports() {
    let source = "class N {}\n\
                  \x20class P<T extends N> {\n\
                  \x20  final T Function()? _create;\n\
                  \x20  P(this._create);\n\
                  \x20  T create() {\n\
                  \x20    return _create;\n\
                  \x20  }\n\
                  \x20}";
    let report = report(source);
    assert_eq!(
        codes(&report, "return_of_invalid_type").len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// The riverpod `provider_listenable_transformer2.dart:237` family's
/// earned twin: the nullable local returned BEFORE any assignment keeps
/// its declared `Sub<T>?` — reports. The assigned-then-returned side
/// (the census line) needs the flow write-promotion; the recovery pair
/// shares this exact (from, to).
#[test]
fn nullable_local_return_before_assignment_reports() {
    let source = "class Sub<T> {}\n\
                  \x20class Outer<T> {\n\
                  \x20  Sub<T> listen() {\n\
                  \x20    Sub<T>? outerSub;\n\
                  \x20    return outerSub;\n\
                  \x20  }\n\
                  \x20}";
    let report = report(source);
    assert_eq!(
        codes(&report, "return_of_invalid_type").len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// The xml `statement.dart:47` / `function.dart:241` family's earned
/// twin: returning the callee OBJECT itself (no invocation) is a genuine
/// `Expr` into `Seq` — reports. The invoked side (the census lines) needs
/// the implicit-`call` face's bare-identifier path; the recovery pair
/// shares this exact (from, to).
#[test]
fn return_of_callee_object_itself_reports() {
    let source = "abstract class Seq {}\n\
                  \x20abstract class Ctx {}\n\
                  \x20abstract class Expr {\n\
                  \x20  Seq call(Ctx context);\n\
                  \x20}\n\
                  \x20Seq use(Expr body, Ctx inner) {\n\
                  \x20  return body;\n\
                  \x20}";
    let report = report(source);
    assert_eq!(
        codes(&report, "return_of_invalid_type").len(),
        1,
        "{:?}",
        report.diagnostics
    );
}
