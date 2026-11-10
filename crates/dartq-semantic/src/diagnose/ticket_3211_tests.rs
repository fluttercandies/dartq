//! The #3211 pins: the `double_literal_coercion_error` multitest face of
//! `integer_literal_imprecise_as_double` — the optional-positional-argument
//! shape the corpus drives (`void notDouble([double value = 0.0])` called
//! with one imprecise integer literal, one variant per corpus section,
//! upstream Dart SDK source,
//! tests/language/double_literals/double_literal_coercion_error_test.dart).
//! The #2870 module pins the predicate and the required-positional faces;
//! these pins lock the same emission through the corpus's exact call
//! shape, including the multitest marker comment surviving on the active
//! line and the trailing comma.

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

/// The imprecise rows of `report` as `(offset, length, message)` tuples —
/// offsets translated back past [`CORE_PRELUDE`] so they address the
/// fixture.
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

/// Variant 001's exact bytes (the active section's file shape): the
/// literal sits on its own argument line with the multitest marker
/// comment intact, the call closes with a trailing comma, and the
/// parameter is the corpus's optional positional
/// (`[double value = 0.0]`). The row anchors at the digits — 16 units,
/// the oracle's `16:5|16` frame — never at the comment's marker text.
#[test]
fn coercion_error_variant_001_shape_reports_at_the_literal() {
    const VARIANT: &str = "\
// Copyright (c) 2018, the Dart project authors.  Please see the AUTHORS file
// for details. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in the LICENSE file.

// Formatting can break multitests, so don't format them.
// dart format off

// Introduces a context with double as type.
void notDouble([double value = 0.0]) {
  if (value != 0.0) throw \"unreachable\";
}

main() {
  notDouble(
    // Large decimal numbers which are not representable as doubles.
    9007199254740993, //     2^53+2^0      //# 001: compile-time error
  );
}
";
    let offset = VARIANT.find("9007199254740993").unwrap() as u32;
    let (row_offset, length, message) = one_imprecise(VARIANT);
    assert_eq!(row_offset, offset);
    assert_eq!(length, 16);
    assert!(
        message.ends_with("without overflow or loss of precision: '9007199254740993'."),
        "{message}"
    );
}

/// The boundary ladder rides the argument face too: 2^53 + 1 reports
/// (variant 001's value), exactly 2^53 stays silent (a representable
/// control the `_none` section carries), and the hex boundary pairs
/// behave the same (`0x20000000000000` silent, `0x20000000000001`
/// reporting — the corpus's hex sections).
#[test]
fn coercion_error_boundary_variants_decide_on_exact_representability() {
    const SHAPE: &str = "void notDouble([double value = 0.0]) {}\nmain() { notDouble(@LIT@,); }\n";
    none_imprecise(&SHAPE.replacen("@LIT@", "9007199254740992", 1));
    let source = SHAPE.replacen("@LIT@", "9007199254740993", 1);
    let (offset, length, _) = one_imprecise(&source);
    assert_eq!(length, 16);
    assert_eq!(offset, source.find("9007199254740993").unwrap() as u32);
    none_imprecise(&SHAPE.replacen("@LIT@", "0x20000000000000", 1));
    let hex = SHAPE.replacen("@LIT@", "0x20000000000001", 1);
    let (offset, length, _) = one_imprecise(&hex);
    assert_eq!(length, 16);
    assert_eq!(offset, hex.find("0x20000000000001").unwrap() as u32);
}

/// The negative sections (`-9007199254740993` as the argument, variant
/// 071): the anchor stays on the digits while the message lexeme carries
/// the minus.
#[test]
fn negated_argument_variant_anchors_at_the_digits() {
    let source =
        "void notDouble([double value = 0.0]) {}\nmain() { notDouble(-9007199254740993,); }\n";
    let (offset, length, message) = one_imprecise(source);
    assert_eq!(length, 16);
    // The digits start one unit past the minus.
    assert_eq!(offset, source.find('-').unwrap() as u32 + 1);
    assert!(message.contains("'-9007199254740993'"), "{message}");
}

/// The 300-digit overflow arm (variant 069, maxFinite + 1): the literal
/// is far past every mantissa frame, the row anchors at the digits, and
/// the correction clamps to maxFinite.
#[test]
fn max_value_plus_one_argument_reports_with_max_finite_correction() {
    const MAX_VALUE_PLUS_ONE: &str = "179769313486231570814527423731704356798070567525844996598917476803157260780028538760589558632766878171540458953514382464234321326889464182768467546703537516986049910576551282076245490090389328944075868508455133942304583236903222948165808559332123348274797826204144723168738177180919299881250404026184124858369";
    let source = format!(
        "void notDouble([double value = 0.0]) {{}}\nmain() {{ notDouble({MAX_VALUE_PLUS_ONE},); }}\n"
    );
    let (offset, length, _) = one_imprecise(&source);
    assert_eq!(length, 309);
    assert_eq!(offset, source.find(MAX_VALUE_PLUS_ONE).unwrap() as u32);
}

/// An `int`-context argument never asks the face (the corpus's silent
/// siblings and the out-of-range arm being a different code): the
/// optional positional typed `int` keeps the huge literal silent.
#[test]
fn int_typed_optional_parameter_stays_silent() {
    none_imprecise("void takeInt([int value = 0]) {}\nmain() { takeInt(9007199254740993,); }\n");
    none_imprecise("void notDouble([double value = 0.0]) {}\nmain() { notDouble(1,); }\n");
}
