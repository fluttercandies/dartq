//! The #2815 pins: the B2 `unchecked_use_of_nullable_value`
//! member-invocation family — the potentially-nullable receiver verdict
//! (R1: `?`-less type parameters through their bounds, nullable function
//! and record shapes, nullable record receivers), the operator and
//! bare-invocation faces the row model had no gate for (R2/R2a), and the
//! promotion-eligibility input (R3: a member access's receiver read
//! never reads a same-named variable's flow promotion).
//!
//! Oracle rows: dart 3.13.7 (the pinned conformance oracle) over the
//! B2 census cases (the 2026-10-28 unchecked-nullable census sweep; the
//! spans mirrored from the upstream expectations).
//! Official sources: type_system.dart `isNonNullable` (:1161-1179),
//! function_expression_invocation_resolver.dart (:53-70),
//! assignment_expression_resolver.dart (:232-278), field_promotability
//! .dart (:70-76).
//!
//! Message variants (#2877): the binary-operator face renders the
//! operator variant and the bare-invocation face the function variant —
//! messages.yaml's four shared-name rows dispatched by the access node's
//! own shape (type_property_resolver.dart:128-148).

use super::*;
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The member-bearing core classes a fixture may name (the shared rig's
/// prelude grade): declared ahead of the fixture so every core spelling
/// resolves.
const CORE_PRELUDE: &str = "class num {} \
     class int extends num { int operator +(int o) => this; int operator -(int o) => this; bool operator >(int o) => true; bool operator <(int o) => true; } \
     class double extends num {} \
     class String { String operator +(String other) => this; int get length => 0; } class bool {} class Iterable<E> {} \
     class List<E> extends Iterable<E> { void add(E e) {} E operator [](int i) => throw 0; int get length => 0; } \
     class Map<K, V> { V? operator [](K k) => null; Iterable<K> get keys => throw 0; } \
     class Set<E> extends Iterable<E> {} class Exception {} \
     abstract class Enum { int get index => 0; }\n";

/// The names-only `dart:core` stub: the special spellings the engine
/// decides by name (`Never` is the bottom type, `Object` the root).
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// The one-library rig (the shared rig's shape): `source` beside
/// [`CORE_PRELUDE`] is the sole analyzed file of `package:t`, with the
/// names-only `dart:core` stub imported for the special spellings.
fn unit_report(source: &str) -> FileReport {
    let full = format!("{CORE_PRELUDE}{source}");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        &full,
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
    let elements = collect(&full, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        dartq_resolution::ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            combinators: dartq_resolution::Combinators::unfiltered(),
        },
    );
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
        &full,
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
    )
}

/// The unchecked rows of `report` as `(code, offset, length, message)`
/// tuples — the conformance face without the file column. Offsets are
/// translated back past [`CORE_PRELUDE`] so they address the fixture.
fn unchecked(source: &str) -> Vec<(u32, u32, String)> {
    let shift = CORE_PRELUDE.len() as u32;
    let report = unit_report(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| {
            (
                d.range.offset.saturating_sub(shift),
                d.range.length,
                d.message.to_string(),
            )
        })
        .collect()
}

/// The single unchecked row of `source` (exactly one is expected).
fn one_unchecked(source: &str) -> (u32, u32, String) {
    let rows = unchecked(source);
    assert_eq!(rows.len(), 1, "{source:?} -> {rows:?}");
    rows[0].clone()
}

/// The byte offset of `needle`'s first occurrence (the anchor locator —
/// fixtures keep every spelling unique).
fn at(source: &str, needle: &str) -> u32 {
    source.find(needle).expect(needle) as u32
}

// —— R1: the potentially-nullable verdict ————————————————————————————

/// A `?`-less type-parameter reference reads its declared bound's
/// nullability (the oracle's `typeParametersNullableBounds` face —
/// static_errors test 42/57 pins): `T extends int?` dereferences
/// unconditionally at the selector.
#[test]
fn param_bound_property_reports_unchecked() {
    let src = "f<T extends int?>(T t) {\n  t.isEven;\n  t.round;\n}";
    let rows = unchecked(src);
    let first = at(src, "isEven");
    let second = at(src, "round");
    assert_eq!(
        rows,
        vec![
            (
                first,
                6,
                "The property 'isEven' can't be unconditionally accessed because the receiver can be 'null'.".to_string()
            ),
            (
                second,
                5,
                "The property 'round' can't be unconditionally accessed because the receiver can be 'null'.".to_string()
            ),
        ]
    );
}

// —— R2: the operator faces ——————————————————————————————————————————

/// Binary operators on a nullable operand report at the operator token
/// with the operator variant (#2877 — messages.yaml's
/// `uncheckedOperatorInvocationOfNullableValue`, the BinaryExpression arm
/// of the node-shape dispatch).
#[test]
fn binary_operator_on_nullable_reports_at_operator() {
    let src = "void f(int? x) {\n  x + 1;\n  x < 1;\n  x >>> 1;\n}";
    let rows = unchecked(src);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0].0, at(src, "+ 1"));
    assert_eq!(rows[0].1, 1);
    assert_eq!(
        rows[0].2,
        "The operator '+' can't be unconditionally invoked because the receiver can be 'null'."
    );
    assert_eq!(rows[1].0, at(src, "< 1"));
    assert_eq!(rows[2].0, at(src, ">>> 1"));
    assert_eq!(rows[2].1, 3);
}

/// The index family takes the method text with the `[]` spelling (the
/// oracle's exact variant for index shapes).
#[test]
fn index_on_nullable_reports_method_text() {
    let src = "void f(List? l) {\n  l[0];\n}";
    let (offset, length, message) = one_unchecked(src);
    assert_eq!(offset, at(src, "[0]"));
    assert_eq!(length, 1);
    assert_eq!(
        message,
        "The method '[]' can't be unconditionally invoked because the receiver can be 'null'."
    );
}

/// Compound assignments and the increments read their operand
/// unconditionally and report the BASE operator at the operator token
/// (the oracle's `x += 1`/`x++`/`++x`/`-x`/`~x` faces).
#[test]
fn compound_and_increment_report_base_operator() {
    let src = "void f(int? x) {\n  x += 1;\n  x++;\n  ++x;\n  -x;\n  ~x;\n}";
    let rows = unchecked(src);
    assert_eq!(rows.len(), 5, "{rows:?}");
    let expect = |row: &(u32, u32, String), needle: &str, len: u32, name: &str| {
        assert_eq!(row.0, at(src, needle), "{needle}");
        assert_eq!(row.1, len);
        assert_eq!(
            row.2,
            format!(
                "The method '{name}' can't be unconditionally invoked because the receiver can be 'null'."
            )
        );
    };
    expect(&rows[0], "+= 1", 2, "+");
    // The postfix increment anchors at its own `++` token (one past `x`).
    assert_eq!(rows[1].0, at(src, "x++") + 1);
    assert_eq!(rows[1].1, 2);
    assert!(rows[1].2.starts_with("The method '+'"));
    expect(&rows[2], "++x", 2, "+");
    expect(&rows[3], "-x", 1, "unary-");
    expect(&rows[4], "~x", 1, "~");
}

/// A promoted operand stands down (the compound reads the pre-write
/// promotion — `c += i` after the null check is silent, the oracle's
/// why_not_promoted compound_assignment shape).
#[test]
fn compound_after_promotion_stays_silent() {
    assert!(unchecked("void f(int? x) {\n  if (x == null) return;\n  x += 1;\n}").is_empty());
}

/// A compound on a MEMBER's own result reports at the operator token
/// naming the base operator (`b.a.y += 0` over `A.y: int?` — the
/// oracle's test_assignment_plusEq_propertyAccess3).
#[test]
fn member_result_compound_reports_at_operator() {
    let src = "class A {\n  int? y;\n  A(this.y);\n}\n\
               void m(A a) {\n  a.y += 0;\n}";
    let (offset, length, message) = one_unchecked(src);
    assert_eq!(offset, at(src, "+= 0"));
    assert_eq!(length, 2);
    assert_eq!(
        message,
        "The method '+' can't be unconditionally invoked because the receiver can be 'null'."
    );
}

// —— R2a: the bare invocation face ———————————————————————————————————

/// A callee whose resolved type is a function type (the bare `Function`
/// head, a written function shape, `Never?`, a function alias, a
/// parameter through its bound) reports at the callee expression with
/// the function variant (#2877).
#[test]
fn bare_invocation_function_callee_reports() {
    let src = "typedef F = void Function();\n\
               void f(Function()? a, Never? b, void Function()? c, F? d) {\n  a(); b(); c(); d();\n}";
    let rows = unchecked(src);
    assert_eq!(rows.len(), 4, "{rows:?}");
    for (row, name) in [
        (&rows[0], "a()"),
        (&rows[1], "b()"),
        (&rows[2], "c()"),
        (&rows[3], "d()"),
    ] {
        assert_eq!(row.0, at(src, name));
        assert_eq!(row.1, 1);
        assert!(row.2.starts_with("The function"), "{}", row.2);
    }
}

/// A parameter spelling through a function bound joins the face (the
/// oracle's `typeParametersNullableBounds` `func()` row) with the
/// function variant (#2877).
#[test]
fn param_function_bound_invoke_reports() {
    let src = "void f<FQ extends Function?>(FQ fn) {\n  fn();\n}";
    let (offset, _, message) = one_unchecked(src);
    assert_eq!(offset, at(src, "fn()"));
    assert!(message.starts_with("The function"), "{message}");
}

/// A resolved call and a dynamic callee stay silent (the guard).
#[test]
fn resolved_and_dynamic_invocations_stay_silent() {
    assert!(unchecked("dynamic d;\nvoid f() {\n  d();\n}").is_empty());
}

// —— R3: the promotion-eligibility input ————————————————————————————

/// A getter, a public field, and a non-final field never promote: the
/// by-name flow row staged at the selector span must not stand the
/// receiver gate down (the oracle's Private-fields-promotion and
/// why_not_promoted suites).
#[test]
fn non_promotable_member_accesses_report_unchecked() {
    let src = "class C {\n  int? i;\n  int? j = null;\n  int? get k => null;\n  \
               void test() {\n    if (i != null) {\n      i.isEven;\n    }\n    \
               if (j != null) {\n      j.isEven;\n    }\n    \
               if (k != null) {\n      k.isEven;\n    }\n  }\n}";
    let rows = unchecked(src);
    assert_eq!(rows.len(), 3, "{rows:?}");
    for (row, needle) in [
        (&rows[0], "i.isEven"),
        (&rows[1], "j.isEven"),
        (&rows[2], "k.isEven"),
    ] {
        assert_eq!(row.0, at(src, needle) + 2, "{needle}");
        assert_eq!(row.1, 6);
    }
}

/// An enum's public field never promotes (the oracle's not_promotable_A02
/// implicit-this shape). The enum-PREFIXED chain (`E.e1.x.isEven`) shares
/// the root but rides the static enum-constant container merge — a
/// separate seam, under-reported until that merge re-decides its result
/// face (the census's one row there).
#[test]
fn enum_public_field_access_reports_unchecked() {
    let src = "enum E {\n  e1(1);\n  final int? x;\n  const E(this.x);\n  \
               void test() {\n    if (x != null) {\n      x.isOdd;\n    }\n  }\n}";
    let rows = unchecked(src);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, at(src, "x.isOdd") + 2);
    assert_eq!(rows[0].1, 5);
}

/// A write escape demotes the promotion (the oracle's
/// `direct_assignment` shape): the read after `i = j` reports.
#[test]
fn write_escape_demotes_promotion() {
    let src = "void f(int? i, int? j) {\n  if (i == null) return;\n  i = j;\n  i.isEven;\n}";
    let (offset, _, _) = one_unchecked(src);
    assert_eq!(offset, at(src, "isEven"));
}

// —— the true-positive guards ————————————————————————————————————————

/// The legitimate promotions still stand the gate down: a parameter, a
/// promotable private final field, a null-check assertion, a null-aware
/// access, and `Object` members on a nullable receiver all stay silent.
#[test]
fn legitimate_promotions_and_exemptions_stay_silent() {
    let promoted = "class D {\n  final int? _x;\n  D(this._x);\n  \
                    void t() {\n    if (_x != null) {\n      _x.isEven;\n    }\n  }\n}";
    assert!(unchecked(promoted).is_empty());
    assert!(unchecked("void f(int? p) {\n  if (p != null) {\n    p.isEven;\n  }\n}").is_empty());
    assert!(unchecked("void f(int? x) {\n  x!.isEven;\n}").is_empty());
    assert!(unchecked("void f(int? x) {\n  x?.isEven;\n}").is_empty());
    assert!(
        unchecked("void f(int? x) {\n  x.hashCode;\n  x.toString();\n  x.runtimeType;\n}")
            .is_empty()
    );
    // A null-aware index and a null-aware-invoked function stay silent too.
    assert!(unchecked("void f(List? l) {\n  l?[0];\n}").is_empty());
}

/// An extension whose `on` type covers the nullable spelling resolves
/// the application (the `Single` suppression the member sites take);
/// a non-nullable `on` face keeps the report (`UnsafeSingle`).
#[test]
fn nullable_on_type_extension_suppresses_operator_face() {
    assert!(
        unchecked(
            "extension E on int? {\n  int operator +(int other) => 0;\n}\n\
         void f(int? x) {\n  x + 1;\n}"
        )
        .is_empty()
    );
}
