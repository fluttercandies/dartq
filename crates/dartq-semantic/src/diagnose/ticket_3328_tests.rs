//! The #3328 pins — the two `isAlwaysExhaustive` arms #3304 left
//! unmodelled on the RESOLVED face (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:846-888):
//!
//! - the `TypeParameterTypeImpl` arm ([:873-879]): a type parameter is
//!   always-exhaustive exactly when its DECLARED bound is, so a switch over
//!   `T extends E` closes like `E` itself;
//! - the `RecordType` arm ([:880-887]): a record splits completely exactly
//!   when every field does, so `(E, F)` over two enums closes and
//!   `(int, E)` does not.
//!
//! Both are under-report arms — the pre-#3328 face answered `false` for
//! every type parameter and every record, so closing one only ever adds a
//! dead tail the oracle already keeps dead. The pins therefore hold two
//! things: the closure (the arm fires) and the negatives that keep the arm
//! honest (an unbounded twin, a spelling two declarations bound
//! differently, an open record field).
//!
//! The first half drives [`ResolvedTypeOps::is_always_exhaustive`]
//! directly — the adapter is the only place the arms live, and a verdict
//! there is the fact the flow layer folds. The second half drives the
//! diagnostic end to end through the shared rig, which mounts the adapter
//! the way the engine does.

use crate::collector::collect;
use crate::diagnose::test_rig::report_ops;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_flow::ty::{ArgTy, Base, EMPTY_ARGS, Ty, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The adapter's verdict for each of `spellings` over a one-file fixture:
/// the spelling resolves through the recorder's lexeme table
/// (`Base::Name(index)` into `spellings`) exactly like a switch scrutinee,
/// and the verdict is [`ResolvedTypeOps::is_always_exhaustive`]'s. The
/// snapshot mounts bare ([`LibraryTypes::build`] — no clause-tree capture),
/// which is deliberate: the bounds these pins ask about ride the element
/// model's `type_param_bounds` columns, not the parse-time capture.
fn exhaustive_verdicts(source: &str, spellings: &[&str]) -> Vec<bool> {
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
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(lib, name, file);
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let names: Vec<Box<str>> = spellings.iter().map(|name| Box::from(*name)).collect();
    let arena: Vec<Vec<ArgTy>> = vec![Vec::new()];
    (0..spellings.len() as u32)
        .map(|id| {
            ops.is_always_exhaustive(
                Ty {
                    base: Base::Name(id),
                    nullable: false,
                },
                EMPTY_ARGS,
                &arena,
                &names,
            )
        })
        .collect()
}

/// Whether the fixture reports the switch-closure row at all — the ERROR
/// twin is the one every non-nullable fixture asks for.
fn reports_completion(source: &str) -> bool {
    report_ops(source).diagnostics.iter().any(|d| {
        d.code.name == "body_might_complete_normally"
            || d.code.name == "body_might_complete_normally_nullable"
    })
}

/// The `TypeParameterTypeImpl` arm over a CONTAINER's own group: `T`
/// bounded by the enum `E` is always-exhaustive, because the bound is
/// (type_system.dart:873-879 — the official recurses into
/// `type.element.bound`).
#[test]
fn container_type_parameter_bound_closes_through_the_enum() {
    let source = "\
enum E { a, b }
class Box<T extends E> {
  void put(T value) {}
}
";
    assert_eq!(
        exhaustive_verdicts(source, &["T"]),
        [true],
        "`T extends E` closes through the enum bound"
    );
}

/// The same arm over a MEMBER's own group (`void m<T extends E>(T t)`):
/// the bound registers from the member entry, not only from the
/// container header.
#[test]
fn member_type_parameter_bound_closes_through_the_enum() {
    let source = "\
enum E { a, b }
class Box {
  void put<T extends E>(T value) {}
}
";
    assert_eq!(
        exhaustive_verdicts(source, &["T"]),
        [true],
        "a method's own `T extends E` closes through the enum bound"
    );
}

/// The sealed-class face of the same arm: the bound's declaration answers
/// through the hierarchy (`ClassElement.isSealed`, :840-843), so
/// `T extends S` over a sealed `S` splits completely.
#[test]
fn type_parameter_bound_closes_through_a_sealed_class() {
    let source = "\
sealed class S {}
class A extends S {}
class B extends S {}
class Box<T extends S> {}
";
    assert_eq!(
        exhaustive_verdicts(source, &["T"]),
        [true],
        "`T extends S` over a sealed `S` is always-exhaustive"
    );
}

/// The soundness pin: the ledger is spelling-keyed within a file, so a
/// second declaration naming the same `T` WITHOUT a bound poisons the
/// entry. A top-level function's group is exactly that shape — `TopDecl`
/// carries its spellings but no bounds — and answering `true` for it
/// would close a switch the oracle leaves open (a false closure, never
/// the under-report the arms are allowed).
#[test]
fn an_unbounded_twin_of_the_same_spelling_keeps_the_parameter_open() {
    let source = "\
enum E { a, b }
class Box<T extends E> {}
void read<T>(T value) {}
";
    assert_eq!(
        exhaustive_verdicts(source, &["T"]),
        [false],
        "an unbounded `T` in the same file keeps the spelling open"
    );
}

/// Two containers bounding one spelling differently poison it for the
/// same reason: `T` names no single declaration's bound here.
#[test]
fn contradicting_bounds_on_one_spelling_keep_the_parameter_open() {
    let source = "\
enum E { a, b }
class A<T extends E> {}
class B<T> {}
";
    assert_eq!(
        exhaustive_verdicts(source, &["T"]),
        [false],
        "two declarations disagreeing on `T`'s bound keep it open"
    );
}

/// The `RecordType` arm (type_system.dart:880-887) over a record alias:
/// every field is closed, so the record splits completely —
/// `isAlwaysExhaustive((E, E))`.
#[test]
fn record_of_closed_fields_splits_completely() {
    let source = "\
enum E { a, b }
typedef Pair = (E, E);
";
    assert_eq!(
        exhaustive_verdicts(source, &["Pair"]),
        [true],
        "a record of two enums is always-exhaustive"
    );
}

/// The record arm's negative: one open field keeps the whole record open.
/// `isAlwaysExhaustive(int)` is `false` (the `int` class is neither sealed
/// nor an enum), so the official answers `false` for `(int, E)` and the
/// tail stays live.
#[test]
fn record_with_an_open_field_keeps_the_tail_live() {
    let source = "\
enum E { a, b }
typedef Mixed = (int, E);
";
    assert_eq!(
        exhaustive_verdicts(source, &["Mixed"]),
        [false],
        "an open record field keeps the record open"
    );
}

/// End to end (the ticket's dominant shape): the tail after a
/// full-constant switch over `T extends E` is unreachable, so the
/// completion row stays silent.
#[test]
fn type_parameter_bounded_by_an_enum_switch_closes_the_tail() {
    let source = "\
enum E { a, b }
int f<T extends E>(T t) {
  switch (t) {
    case E.a: return 1;
    case E.b: return 2;
  }
}
";
    assert!(
        !reports_completion(source),
        "`T extends E` over a fully covered enum closes the tail: {:?}",
        report_ops(source).diagnostics
    );
}

/// End-to-end negative control: an UNBOUNDED `T` reads `Object?`, whose
/// element face is an open class — the same switch still reports the
/// completion row, so the arm must not answer `true` for every parameter.
#[test]
fn unbounded_type_parameter_switch_keeps_the_completion() {
    let source = "\
enum E { a, b }
int f<T>(T t) {
  switch (t) {
    case E.a: return 1;
    case E.b: return 2;
  }
}
";
    assert!(
        reports_completion(source),
        "an unbounded `T` keeps the tail live: {:?}",
        report_ops(source).diagnostics
    );
}
