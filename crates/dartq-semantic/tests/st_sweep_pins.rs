//! Burn pins for the ST-sweep wave: the N2 declaration face (#1595) and the
//! N3 expression/invocation face (#1596) rows of the divergence burndown's
//! "semantic no-emitter sweep" cluster.
//!
//! Each pin replays one regression case's oracle golden
//! (`tests/corpus/regressions/cases/<id>.json`, the 3.13.1 measurement)
//! against the live pipeline in the corpus posture — language 3.13, the
//! `augmentations` experiment off (`augment` lexes as an identifier, the
//! recovery shapes the cases exercise), a standalone library with the
//! synthetic dart:core name face. The pin set per case is exactly the
//! wave's row list plus its declaration riders, and the
//! assertion is set parity on `(code, line, column)` per pinned code:
//! over- and under-reports both fail.
//!
//! The pinned code set per case is the burn-wave row list for this wave
//! plus its declaration-rider codes (`final_not_initialized` /
//! `const_not_initialized` on the augmentation cases) — both families are
//! self-contained here, so the pins survive without the wave's internal
//! planning documents.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::collector::collect;
use dartq_semantic::diagnose::{StrictModes, fold_unit};
use dartq_semantic::library::ElementIndex;
use dartq_source::{FileTable, StringTable};

/// Repo-relative corpus root, from this crate's manifest.
const CORPUS_ROOT: &str = "../../tests/corpus";

/// The synthetic dart:core name face: the top-level names these fixtures
/// consume (the engine's implicit core import resolves the same spellings;
/// the face exists so core-typed values keep their static types — an absent
/// `List` would type the for-each error-face and suppress its condition
/// check). Same provenance posture as the diagnose unit tests' SDK faces.
const CORE_FACE: &[&str] = &[
    "bool",
    "Comparable",
    "Comparator",
    "double",
    "Enum",
    "Error",
    "Exception",
    "Function",
    "Future",
    "int",
    "Invocation",
    "Iterable",
    "Iterator",
    "List",
    "Map",
    "MapEntry",
    "Match",
    "Never",
    "Null",
    "num",
    "Object",
    "Pattern",
    "print",
    "Record",
    "Set",
    "Sink",
    "StackTrace",
    "String",
    "Symbol",
    "Type",
    "identical",
];

/// One pinned case: the regression id, its corpus file, and the pinned
/// codes (the wave's burn rows plus the declaration riders named in the
/// module docs).
struct Case {
    id: &'static str,
    file: &'static str,
    codes: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        id: "4093e6ac",
        file: "m0-syntax/lib/src/error_recovery/keyword_named_class_methods_2.dart",
        codes: &[
            "invalid_use_of_null_value",
            "invocation_of_non_function_expression",
            "non_bool_condition",
            "not_a_type",
            // The record-literal operator face (#1748): `const(x-1) + 1`
            // folds the one-positional literal into a record-typed row and
            // the `+` miss resolves against it — oracle row 49:23, the
            // record display `(int,)`.
            "undefined_operator",
        ],
    },
    Case {
        id: "6e0cf0a2",
        file: "m0-syntax/lib/src/error_recovery/keyword_named_top_level_methods_2.dart",
        codes: &[
            "invalid_reference_to_this",
            "invalid_use_of_null_value",
            "invocation_of_non_function_expression",
            "non_bool_condition",
            "not_a_type",
            "super_in_invalid_context",
        ],
    },
    Case {
        id: "666ddbb1",
        file: "m0-syntax/lib/src/augmentation/member_errors.dart",
        codes: &[
            "conflicting_static_and_instance",
            "const_instance_field",
            "non_void_return_for_setter",
            "final_not_initialized",
            "const_not_initialized",
        ],
    },
    Case {
        id: "7c5ae0fd",
        file: "m0-syntax/lib/src/augmentation/member_declarations.dart",
        codes: &[
            "const_instance_field",
            "non_void_return_for_setter",
            "final_not_initialized",
        ],
    },
    Case {
        id: "8b3fc913",
        file: "m0-syntax/lib/src/augmentation/top_level_declarations.dart",
        codes: &[
            "non_void_return_for_setter",
            "not_a_type",
            "final_not_initialized",
        ],
    },
    Case {
        id: "e863c628",
        file: "m0-syntax/lib/src/augmentation/top_level_errors.dart",
        codes: &["non_void_return_for_setter", "not_a_type"],
    },
    Case {
        id: "f600b4ad",
        file: "m0-syntax/lib/src/general/issue_45703.dart",
        codes: &["not_a_type"],
    },
    Case {
        id: "a49cceb1",
        file: "m0-syntax/lib/targeted/blindspot_function_types.dart",
        codes: &["non_bool_condition"],
    },
    Case {
        id: "7bb9b1bc",
        file: "m0-syntax/lib/src/general/function_reference_following_token.dart",
        codes: &["non_bool_negation_expression"],
    },
    Case {
        id: "9497f84a",
        file: "m0-syntax/lib/src/general/call_on_after_try_block2.dart",
        codes: &["non_type_in_catch_clause", "undefined_class"],
    },
    // The qualified-creation head face (#1748): `a.A.new(...)` /
    // `a.A<int>.new(...)` resolve `.new` as the method face of the implicit
    // creation (`type_property_resolver` `needsGetterError`), so the dead
    // prefix reports `undefined_method` on the `<unknown>`-typed receiver.
    // The `Created` receiver's null-aware face rides the same pin
    // (`C2()?..new()` at 61 — a creation's result is its non-nullable
    // container, `_checkForUnnecessaryNullAware`).
    Case {
        id: "d1b3378f",
        file: "m0-syntax/lib/src/general/new_as_identifier.dart",
        codes: &["undefined_method", "invalid_null_aware_operator"],
    },
    // The function-typed formal face (#1758): `f()?.x` on `int f()`-shaped
    // parameters — the formal's name binds through the `end_formal_parameter`
    // arm (the parser's inline-name path fires no identifier event), and the
    // null-aware check reads the parameter's return-type slot.
    Case {
        id: "d194820a",
        file: "m0/lib/flow_analysis/reachability/data/null_aware_access.dart",
        codes: &["dead_code", "invalid_null_aware_operator"],
    },
];

/// Documented residuals: rows the oracle carries but this engine
/// deliberately does not, each attributed outside this crate's face. The
/// pins assert parity modulo this table — removing an entry without
/// closing its root cause fails the pin.
/// - (`666ddbb1`, final_not_initialized, 32:26): `external augment final
///   field;` — the final-field face is parser-owned (the declaration
///   walk), and the external-augment recovery shape drops the row there;
///   a parser-lane rider closes it.
/// - (`f600b4ad`, not_a_type, 14:3): `Function ok;` — the bare `Function`
///   keyword head (no `<`/`(` follows) fires no type-reference event in
///   the parser, so no type-position row exists for the semantic layer to
///   classify; the oracle resolves the head as a NamedType and reports.
///   A parser-lane rider closes it.
const RESIDUALS: &[(&str, &str, (usize, usize))] = &[
    ("666ddbb1", "final_not_initialized", (32, 26)),
    ("f600b4ad", "not_a_type", (14, 3)),
];

/// The golden's rows for `code`: `(line, column)` pairs, 1-based.
fn golden_rows(case_id: &str, code: &str) -> Vec<(usize, usize)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(CORPUS_ROOT)
        .join("regressions/cases")
        .join(format!("{case_id}.json"));
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let value: serde_json::Value = serde_json::from_str(&text).expect("golden json");
    value["oracle"]
        .as_array()
        .expect("oracle rows")
        .iter()
        .filter(|row| row["code"].as_str() == Some(code))
        .map(|row| {
            (
                row["line"].as_u64().expect("line") as usize,
                row["column"].as_u64().expect("column") as usize,
            )
        })
        .collect()
}

/// Runs `source` through the live pipeline in the corpus posture and
/// returns the merged report (parse rows + semantic rows, the production
/// halves; set parity needs no ordering).
fn unit_report(source: &str) -> dartq_semantic::FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    // The corpus pins `sdk: ^3.13.0` with no experiment flag: `augment`
    // lexes as an identifier, exactly the recovery shapes these cases carry.
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 13));
    let scanned = scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(dartq_language::Feature::Augmentations),
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {scanned:?}"
    );
    let features = dartq_parser::ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_file = files.intern(Path::new("<dart:core>"));
    let core = builder.intern("dart:core");
    for name in CORE_FACE {
        builder.declare(core, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == dartq_semantic::MicroKind::TypeParam {
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
        &dartq_resolution::LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    let mut diagnostics = fold.parse_diagnostics;
    diagnostics.extend(report.diagnostics.drain(..));
    report.diagnostics = diagnostics;
    report
}

/// The 1-based line/column of a diagnostic offset in `source`.
fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let line = source[..offset].bytes().filter(|b| *b == b'\n').count() + 1;
    let column = offset - source[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0) + 1;
    (line, column)
}

/// The pin body: set parity per pinned code, one assembled diff message.
fn assert_case(case: &Case) {
    let source_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(CORPUS_ROOT)
        .join(case.file);
    let source = fs::read_to_string(&source_path).unwrap_or_else(|e| panic!("reading case: {e}"));
    let report = unit_report(&source);

    let mut diff = String::new();
    let is_residual = |code: &str, pos: (usize, usize)| {
        RESIDUALS
            .iter()
            .any(|(id, c, r)| *id == case.id && *c == code && *r == pos)
    };
    for code in case.codes {
        let oracle: BTreeSet<(usize, usize)> = golden_rows(case.id, code)
            .into_iter()
            .filter(|pos| !is_residual(code, *pos))
            .collect();
        let dartq: BTreeSet<(usize, usize)> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == *code)
            .map(|d| line_col(&source, usize::try_from(d.range.offset).unwrap()))
            .filter(|pos| !is_residual(code, *pos))
            .collect();
        if oracle == dartq {
            continue;
        }
        diff.push_str(&format!(
            "  {code}: oracle {:?} != dartq {:?}\n",
            oracle, dartq
        ));
    }
    assert!(
        diff.is_empty(),
        "case {} {} diverged:\n{diff}",
        case.id,
        case.file
    );
}

#[test]
fn st_sweep_4093e6ac_keyword_named_class_methods_2() {
    assert_case(&CASES[0]);
}

#[test]
fn st_sweep_6e0cf0a2_keyword_named_top_level_methods_2() {
    assert_case(&CASES[1]);
}

#[test]
fn st_sweep_666ddbb1_augmentation_member_errors() {
    assert_case(&CASES[2]);
}

#[test]
fn st_sweep_7c5ae0fd_augmentation_member_declarations() {
    assert_case(&CASES[3]);
}

#[test]
fn st_sweep_8b3fc913_augmentation_top_level_declarations() {
    assert_case(&CASES[4]);
}

#[test]
fn st_sweep_e863c628_augmentation_top_level_errors() {
    assert_case(&CASES[5]);
}

#[test]
fn st_sweep_f600b4ad_issue_45703() {
    assert_case(&CASES[6]);
}

#[test]
fn st_sweep_a49cceb1_blindspot_function_types() {
    assert_case(&CASES[7]);
}

#[test]
fn st_sweep_7bb9b1bc_function_reference_following_token() {
    assert_case(&CASES[8]);
}

#[test]
fn st_sweep_9497f84a_call_on_after_try_block2() {
    assert_case(&CASES[9]);
}

#[test]
fn st_sweep_d1b3378f_new_as_identifier() {
    assert_case(&CASES[10]);
}

#[test]
fn st_sweep_d194820a_null_aware_access() {
    assert_case(&CASES[11]);
}
