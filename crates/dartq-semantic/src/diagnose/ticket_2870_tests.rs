//! The #2870 pins: the `integer_literal_imprecise_as_double` face — the
//! exact-representability predicate the int→double literal conversion
//! answers (`IntegerLiteralImpl.isValidAsDouble`, upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/ast/ast.dart:19479), the three
//! emission faces the conversion grant routes through (declaration
//! initializers, plain assignments, call arguments — `_checkForOutOfRange`'s
//! double arm, error_verifier.dart:6789), and the anchor/negation/message
//! shapes (the literal token only, the lexeme `-`-prefixed under an
//! immediately-negated operand, the nearest valid double in the
//! correction slot).
//!
//! Oracle rows: dart 3.13.7 (the pinned conformance oracle) — the official
//! `IntegerLiteralImpreciseAsDoubleTest` rows, the co19 `int-to-double`
//! representation/min/max rows, and the `double_literal_coercion_error`
//! multitest arguments (census rank 4: 292 fail rows, pure
//! under-detection).

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
/// decides by name.
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
            non_library: false,
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

/// The imprecise rows of `report` as `(offset, length, message)`
/// tuples — offsets translated back past [`CORE_PRELUDE`] so they
/// address the fixture.
fn imprecise(source: &str) -> Vec<(u32, u32, String)> {
    let shift = CORE_PRELUDE.len() as u32;
    unit_report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "integer_literal_imprecise_as_double")
        .map(|d| {
            (
                d.range.offset.saturating_sub(shift),
                d.range.length,
                d.message.to_string(),
            )
        })
        .collect()
}

/// The single imprecise row of `source` (exactly one is expected).
fn one_imprecise(source: &str) -> (u32, u32, String) {
    let rows = imprecise(source);
    assert_eq!(rows.len(), 1, "{source:?} -> {rows:?}");
    rows[0].clone()
}

/// The face stays silent over `source`.
fn none_imprecise(source: &str) {
    let rows = imprecise(source);
    assert!(rows.is_empty(), "{source:?} -> {rows:?}");
}

// —— the predicate port (the pure digit-string arithmetic) ————————————

/// `isValidAsDouble`'s boundary ladder, pinned against the upstream
/// examples: 2^53 − 1 / 2^53 fit, 2^53 + 1 doesn't; the hex boundary
/// mirrors it (`0x20000000000000` = 2^53); a dropped-mantissa tail of
/// zeros stays exact at any width; the maxFinite frame decides the
/// overflow arm.
#[test]
fn predicate_boundaries_match_upstream() {
    // Below the 16-character shortcut nothing reaches the arithmetic.
    assert!(super::valid_as_double("900719925474099"));
    // 2^53 − 1 (16 chars, 53 bits).
    assert!(super::valid_as_double("9007199254740991"));
    // 2^53: 54 bits, the dropped tail is the single zero bit.
    assert!(super::valid_as_double("9007199254740992"));
    // 2^53 + 1: the dropped tail carries the set bit (the official
    // test_excessiveMantissa neighbor at the smallest failing width).
    assert!(!super::valid_as_double("9007199254740993"));
    // 2^62 and its −2^5 sibling (the coercion_error 009/010 pair).
    assert!(super::valid_as_double("4611686018427387904"));
    assert!(!super::valid_as_double("4611686018427387872"));
    // 2^63: the official test's representable control.
    assert!(super::valid_as_double("9223372036854775808"));
    assert!(!super::valid_as_double("9223372036854775809"));
    // The hex boundary: 2^53 fits, +1 and the 54-bit odd tail don't
    // (the co19 representation_fail_t02/t03 shapes).
    assert!(super::valid_as_double("0x20000000000000"));
    assert!(!super::valid_as_double("0x20000000000001"));
    assert!(!super::valid_as_double("0x2fffffffffffff"));
    // maxFinite itself is representable (54-bit run? no — 1024 bits with
    // a 971-bit zero tail); +1 overflows the exponent arm.
    let max_finite = "179769313486231570814527423731704356798070567525844996598917476803157260780028538760589558632766878171540458953514382464234321326889464182768467546703537516986049910576551282076245490090389328944075868508455133942304583236903222948165808559332123348274797826204144723168738177180919299881250404026184124858368";
    assert!(super::valid_as_double(max_finite));
    let max_finite_plus_one: String = {
        let mut digits: Vec<u8> = max_finite.bytes().map(|b| b - b'0').collect();
        let last = digits.len() - 1;
        digits[last] += 1;
        digits.into_iter().map(|d| char::from(d + b'0')).collect()
    };
    assert!(!super::valid_as_double(&max_finite_plus_one));
}

/// `nearestValidDouble`'s rendering: the rounded value spelled as the
/// exact integer (the upstream correction's `BigInt.from(...).toString()`
/// face), and the overflow clamp to maxFinite.
#[test]
fn nearest_double_renders_exact_integers() {
    assert_eq!(
        super::nearest_valid_double("9223372036854775809"),
        "9223372036854775808"
    );
    assert_eq!(
        super::nearest_valid_double("9007199254740993"),
        "9007199254740992"
    );
    let max_finite = "179769313486231570814527423731704356798070567525844996598917476803157260780028538760589558632766878171540458953514382464234321326889464182768467546703537516986049910576551282076245490090389328944075868508455133942304583236903222948165808559332123348274797826204144723168738177180919299881250404026184124858368";
    // Beyond the frame the suggestion clamps to maxFinite itself.
    assert_eq!(
        super::nearest_valid_double(
            "179769313486231570814527423731704356798070567525844996598917476803157260780028538760589558632766878171540458953514382464234321326889464182768467546703537516986049910576551282076245490090389328944075868508455133942304583236903222948165808559332123348274797826204144723168738177180919299881250404026184124858369"
        ),
        max_finite
    );
}

// —— the declaration-initializer face ——————————————————————————————————

/// The doc row (`double x = 9223372036854775807;`): fires at the literal,
/// the message spells the lexeme, the correction suggests the rounded
/// double.
#[test]
fn imprecise_declaration_initializer_reports() {
    let (offset, length, message) = one_imprecise("double x = 9223372036854775807;\n");
    assert_eq!(offset, 11);
    assert_eq!(length, 19);
    assert!(
        message.ends_with("without overflow or loss of precision: '9223372036854775807'."),
        "{message}"
    );
}

/// The exactly-representable controls stay silent: 2^53 in decimal, 2^53
/// in hex, 2^63 (the official test's excessive-mantissa boundary
/// neighbor), and the sub-boundary widths the 16-character shortcut
/// covers.
#[test]
fn representable_initializers_stay_silent() {
    none_imprecise("double x = 9007199254740992;\n");
    none_imprecise("double x = 0x20000000000000;\n");
    none_imprecise("double x = 9223372036854775808;\n");
    none_imprecise("double x = 1;\n");
}

/// The co19 hex shapes (`representation_fail_t02/t03`): the 54-bit odd
/// tails report.
#[test]
fn hex_54bit_tails_report() {
    let (offset, length, message) = one_imprecise("double x = 0x2fffffffffffff;\n");
    assert_eq!((offset, length), (11, 16));
    assert!(message.contains("'0x2fffffffffffff'"), "{message}");
    one_imprecise("double x = 0x20000000000001;\n");
}

/// The immediately-negated face (`representation_fail_t06`,
/// `min_acceptable_values_fail_t01`): the anchor stays on the digits
/// (never the minus) while the message lexeme carries it.
#[test]
fn negated_literal_anchors_at_the_digits() {
    let (offset, length, message) = one_imprecise("double x = -9007199254740993;\n");
    // `double x = ` is 11 columns; the minus sits at 11, the digits at 12.
    assert_eq!(offset, 12);
    assert_eq!(length, 16);
    assert!(message.contains("'-9007199254740993'"), "{message}");
}

/// The digit separators ride the message lexeme untouched and strip out
/// of the predicate (`test_excessiveMantissa_withSeparators`).
#[test]
fn separators_report_with_the_written_lexeme() {
    let (offset, length, message) = one_imprecise("double x = 9_223_372_036_854_775_809;\n");
    assert_eq!(offset, 11);
    assert_eq!(length, 25);
    assert!(message.contains("'9_223_372_036_854_775_809'"), "{message}");
}

/// The conversion keys on the literal, never the value's context alone:
/// a non-literal `int` source in a double slot keeps the old silence
/// (`double d = intVar;`), and non-double contexts (`num`, `dynamic`,
/// inferred `var`, and the `int` slot whose out-of-range row is a
/// different code) never ask the face.
#[test]
fn non_literal_and_non_double_contexts_stay_silent() {
    none_imprecise("void f() { int v = 1; double d = v; }\n");
    none_imprecise("num x = 9007199254740993;\n");
    none_imprecise("dynamic x = 9007199254740993;\n");
    none_imprecise("var x = 9007199254740993;\n");
    none_imprecise("int x = 9007199254740993;\n");
}

// —— the assignment and argument faces —————————————————————————————————

/// The plain assignment face (`double d; d = 9223372036854775807;`):
/// the same double context one statement later.
#[test]
fn imprecise_assignment_reports() {
    let (offset, length, _) = one_imprecise("void f() { double d; d = 9223372036854775807; }\n");
    assert_eq!(length, 19);
    assert_eq!(offset, 25);
}

/// The call-argument face (the `double_literal_coercion_error`
/// multitest's `notDouble(<literal>)` shape, 282 corpus rows): fires at
/// the argument literal, the negated form anchors at the digits.
#[test]
fn imprecise_argument_reports() {
    let (offset, length, message) =
        one_imprecise("void f(double v) {}\nvoid g() { f(9007199254740993); }\n");
    assert_eq!(length, 16);
    assert_eq!(offset, 33);
    assert!(message.contains("'9007199254740993'"), "{message}");
    let (offset, _, message) =
        one_imprecise("void f(double v) {}\nvoid g() { f(-9007199254740993); }\n");
    assert_eq!(offset, 34);
    assert!(message.contains("'-9007199254740993'"), "{message}");
}

/// The representable argument stays silent (the coercion multitest's
/// pass faces: exact powers of two are legal doubles).
#[test]
fn representable_argument_stays_silent() {
    none_imprecise("void f(double v) {}\nvoid g() { f(4611686018427387904); }\n");
    none_imprecise("void f(double v) {}\nvoid g() { f(0.5); }\n");
}

/// The static-field and top-level initializers route the same
/// declaration face (`representation_fail_t01`'s four anchors: top-level,
/// static, and local declarations all report).
#[test]
fn field_and_top_level_initializers_report() {
    let rows = imprecise(
        "double t = 18446744073709551615;\nclass C { static double s = 18446744073709551615; }\n",
    );
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].1, 20);
    assert_eq!(rows[1].1, 20);
}
