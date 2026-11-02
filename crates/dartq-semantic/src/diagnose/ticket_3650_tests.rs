//! #3650 — augmentation-face recon pins: per-code oracle-verbatim tuples for
//! the census sub-families that under- or over-report.
//!
//! Every expectation below is a verbatim oracle 3.13.7 shape taken from the
//! conformance corpus (`diagnostics/augmentation_*` test directories and the
//! co19 `LanguageFeatures/Augmentations` sweep):
//!
//! ```text
//! typedef A = int; augment class A {}        -> AUGMENTATION_OF_DIFFERENT_DECLARATION_KIND @ `augment`
//! class B {} augment class B extends A {}    -> silent (intro has no extends clause)
//! void f<T>(); augment void f<T>() {}        -> silent (same arity/names)
//! void f();   augment void f<T>() {}         -> AUGMENTATION_TYPE_PARAMETER_COUNT @ `T`
//! void f([int i = 0]); augment void f({int i});  -> AUGMENTATION_OPTIONAL_POSITIONAL... + NAMED_EXTRA
//! set foo(int _); augment int foo = 1;       -> AUGMENTATION_WITHOUT_GETTER_DECLARATION + INDUCED_SETTER_ALREADY_COMPLETE
//! ```

use crate::FileReport;
use crate::diagnose::test_rig::{
    CORE_PRELUDE, RigImport, report_ops_augment_experiments, report_ops_augment_intro_experiments,
    report_ops_experiments_with,
};
use dartq_language::Feature;

/// The fixture's rows of one code as `(offset, length)`, ascending.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut out = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect::<Vec<_>>();
    out.sort_unstable();
    out
}

/// Every produced row, for failure output: (code, offset, len, message).
fn dump(report: &FileReport) -> Vec<(&'static str, u32, u32, &str)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message.as_str(),
            )
        })
        .collect()
}

/// A fixture offset in a prelude-bearing unit (`same()` and `aug()`'s
/// main unit both mount `CORE_PRELUDE` ahead of the source).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("anchor")) as u32
}

/// A fixture offset in the AUGMENTING unit — the rig mounts its source
/// verbatim (no prelude).
fn at_aug(source: &str, needle: &str) -> u32 {
    source.find(needle).expect("anchor") as u32
}

/// The two-unit rig, experiment on, reading the AUGMENTING unit's report.
fn aug(main: &str, augment: &str) -> FileReport {
    report_ops_augment_experiments(main, augment, &[Feature::Augmentations])
}

/// The single-unit rig, experiment on — same-file intro + augmentation.
/// The prelude mounts so `int`/`String`/`num` resolve as types.
fn same(source: &str) -> FileReport {
    report_ops_experiments_with(
        &format!("{CORE_PRELUDE}{source}"),
        &[],
        None,
        &[Feature::Augmentations],
    )
}

const KIND: &str = "augmentation_of_different_declaration_kind";
const WO: &str = "augmentation_without_declaration";
const TPC: &str = "augmentation_type_parameter_count";
const TPN: &str = "augmentation_type_parameter_name";
const TPB: &str = "augmentation_type_parameter_bound";
const RTM: &str = "augmentation_return_type_mismatch";
const FTM: &str = "augmentation_formal_parameter_type_mismatch";
const EXT: &str = "augmentation_extends_clause_already_present";
const OPC: &str = "augmentation_optional_positional_formal_parameter_count";
const NFE: &str = "augmentation_named_formal_parameter_extra";
const NFM: &str = "augmentation_named_formal_parameter_missing";
const DAC: &str = "declaration_already_complete";
const DVC: &str = "default_value_already_specified_in_augmentation_chain";
const CVA: &str = "constant_variable_augmentation";

// ── of_different_declaration_kind: typedef target ────────────────────────
// Oracle: `augment class A {}` over `typedef A = int;` reports the
// different-kind refusal at the `augment` keyword — the typedef fragment
// joins `lastFragments` upstream, so the refusal reads `type alias`.

#[test]
fn augment_class_over_typedef_reports_different_kind() {
    let source = "typedef A = int;\naugment class A {}\n";
    let report = same(source);
    assert_eq!(
        rows(&report, KIND),
        vec![(at(source, "augment"), 7)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, WO),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn augment_extension_type_over_typedef_reports_different_kind() {
    let source = "typedef A = int;\naugment extension type A(int i) {}\n";
    let report = same(source);
    assert_eq!(
        rows(&report, KIND),
        vec![(at(source, "augment"), 7)],
        "{:?}",
        dump(&report)
    );
}

// ── augmentation_extends_clause_already_present ──────────────────────────
// Oracle (`AugmentationExtendsClauseAlreadyPresentTest/test_notPresent`):
// `class B {}` + `augment class B extends A {}` is SILENT — the intro's own
// `extends` absence means the aug legally supplies it.

#[test]
fn extends_clause_silent_when_intro_has_none() {
    let main = "class A {}\nclass B {}\n";
    let augment = "augment class B extends A {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, EXT),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn extends_clause_silent_when_intro_has_none_same_file() {
    let source = "class A {}\nclass B {}\naugment class B extends A {}\n";
    let report = same(source);
    assert_eq!(
        rows(&report, EXT),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn extends_clause_reports_when_intro_has_one() {
    let main = "class A {}\nclass B extends A {}\n";
    let augment = "augment class B extends A {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, EXT),
        vec![(at_aug(augment, "extends"), 7)],
        "{:?}",
        dump(&report)
    );
}

// ── type parameters (top-level function augment) ─────────────────────────
// Oracle (`AugmentationTypeParameterCountTest/test_topLevelFunction_*`):
// count fires per shape — surplus anchors at the extra name, matching
// names/bounds stay silent.

// The top-level type-parameter capture reads `builder.pending_top` AFTER
// its `.take()` (collector.rs:5028-5033 — the aug block runs inside the
// `if let Some(pending)` scope but reloads the drained slot), so
// `AugmentDecl.type_params` is always empty for top-level functions: every
// generic augment reports `type_parameter_count` at its name and its
// type-parameter names never join the param scope (the `T` vs `T`
// `formal_parameter_type_mismatch` extras). Rider: read
// `pending.type_param_names`/`pending.type_param_bounds` before they move
// into `TopDecl`.

#[test]
fn type_param_count_silent_when_aug_matches_intro() {
    let main = "void f<T>(T t) {}\n";
    let augment = "augment void f<T>(T t) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, TPC),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, TPN),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn type_param_count_anchors_at_surplus_name() {
    // Oracle `test_topLevelFunction_1_2`: intro `f<T>`, aug `f<T, U>` —
    // one row at the surplus `U` (len 1), anchored at its name token.
    let main = "void f<T>(T t) {}\n";
    let augment = "augment void f<T, U>(T t) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, TPC),
        // `U` sits two characters after the `,` in `<T, U>` — anchor len 1.
        vec![(at_aug(augment, ", U") + 2, 1)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn type_param_count_anchors_at_aug_name_when_aug_writes_none() {
    // Oracle `test_topLevelFunction_1_1`-family (aug `f()` over `f<T>()`):
    // one row at the augmentation's own name span.
    let main = "void f<T>(T t) {}\n";
    let augment = "augment void f(T t) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, TPC),
        vec![(at_aug(augment, "f("), 1)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn type_param_name_reports_at_aug_param_name() {
    // Oracle `test_topLevelFunction_T_U`: intro `f<T>`, aug `f<U>` — one
    // name-mismatch row at the aug's `U`, no count row.
    let main = "void f<T>(T t) {}\n";
    let augment = "augment void f<U>(U u) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, TPN),
        vec![(at_aug(augment, "U>"), 1)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, TPC),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn type_param_bound_reports_at_aug_bound_type() {
    // Oracle `test_topLevelFunction_num_int`: intro `f<T extends num>`,
    // aug `f<T extends int>` — one bound row at the aug's `int`.
    let main = "void f<T extends num>(T t) {}\n";
    let augment = "augment void f<T extends int>(T t) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, TPB),
        vec![(at_aug(augment, "int>"), 3)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, TPC),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

// ── return-type / formal-type comparisons ────────────────────────────────

#[test]
fn return_type_mismatch_anchors_at_written_type() {
    // Oracle `test_topLevelFunction_void_int`: intro `void f()`, aug
    // `int f()` — one row at the aug's `int` spelling.
    let main = "void f() {}\n";
    let augment = "augment int f() {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, RTM),
        vec![(at_aug(augment, "int"), 3)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn return_type_mismatch_anchors_at_prefixed_type() {
    // Oracle `test_topLevelFunction_void_int_withImportPrefix`: aug
    // `core.int f()` — the anchor spans the WHOLE qualified spelling
    // (`core.int`, len 8), not the head token. Body-less augmentation so
    // `declaration_already_complete` stays out of the signal.
    let source = "void f() {}\naugment core.int f();\n";
    let report = report_ops_experiments_with(
        source,
        &[RigImport {
            uri: "dart:core",
            prefix: Some("core"),
            source: "class int {}",
        }],
        None,
        &[Feature::Augmentations],
    );
    assert_eq!(
        rows(&report, RTM),
        vec![(at_aug(source, "core.int"), 8)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn formal_type_mismatch_reports_at_aug_param_type() {
    // Oracle `AugmentationFormalParameterTypeTest`: intro `f(int p)`,
    // aug `f(String p)` — one row anchored at the aug's `String`.
    let main = "void f(int p) {}\n";
    let augment = "augment void f(String p) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        vec![(at_aug(augment, "String"), 6)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn formal_type_equal_type_params_silent() {
    // Oracle `augmenting_functions_A04_t01`: intro `f<T extends String>(T)`,
    // aug `f<T extends String>(T)` — the param pair `T` vs `T` is EQUAL
    // (both signature variables), so no mismatch row. The augmentation is
    // body-less so `declaration_already_complete` stays out of the signal.
    let main = "String f<T extends String>(T t) => t;\n";
    let augment = "augment T f<T extends String>(T t);\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, TPC),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

// ── optional-positional / named shape ────────────────────────────────────

#[test]
fn optional_positional_count_reports_at_right_paren() {
    // Oracle: aug `f({int i})` over intro `f([int i = 0])` — the optional
    // positional shape differs (0 vs 1) → one count row at `)`, and the
    // named-extra row for `i` (upstream runs the named pass regardless).
    let main = "void f([int i = 0]) {}\n";
    let augment = "augment void f({int i}) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, OPC),
        vec![(at_aug(augment, ")"), 1)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, NFE),
        vec![(at_aug(augment, "i}"), 1)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn named_formal_missing_reports_at_right_paren() {
    // Oracle: aug `f()` over intro `f({int i})` — the named `i` is absent
    // → one missing row at `)`.
    let main = "void f({int i}) {}\n";
    let augment = "augment void f() {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, NFM),
        vec![(at_aug(augment, ")"), 1)],
        "{:?}",
        dump(&report)
    );
}

// ── induced accessors (variable-family) ─────────────────────────────────

#[test]
fn setter_only_intro_induces_missing_getter() {
    // Oracle `test_class_instanceSetter_hasBody_augmentation_instanceField`:
    // `set foo(int _)` + `augment int foo = 1` reports
    // `augmentation_without_declaration` (getter shape) at the aug's name
    // AND `augmentation_induced_setter_already_complete` at the same name.
    let main = "class A {\n  set foo(int _) {}\n}\n";
    let augment = "augment class A {\n  augment int foo = 1;\n}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, WO),
        vec![(at_aug(augment, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, DAC),
        vec![(at_aug(augment, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn getter_only_intro_induces_missing_setter() {
    // Oracle `test_class_instanceGetter_hasBody_augmentation_instanceField`:
    // `int get foo` + `augment int foo = 1` reports the induced-setter
    // without-declaration row plus the induced-getter already-complete row.
    let main = "class A {\n  int get foo => 0;\n}\n";
    let augment = "augment class A {\n  augment int foo = 1;\n}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, WO),
        vec![(at_aug(augment, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, DAC),
        vec![(at_aug(augment, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

// ── default_value_already_specified_in_augmentation_chain ────────────────

#[test]
fn default_value_chain_reports_at_separator() {
    // Oracle `test_topLevelFunction_optionalPositional`: intro
    // `f([int p = 0])`, aug `f([int p = 1])` — the aug's optional
    // positional formal re-specifies a default its chain already carried →
    // one row at the `=` separator.
    let main = "void f([int p = 0]) {}\n";
    let augment = "augment void f([int p = 1]) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, DVC),
        vec![(at_aug(augment, "= 1"), 1)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn default_value_chain_silent_when_intro_has_none() {
    let main = "void f([int? p]) {}\n";
    let augment = "augment void f([int p = 1]) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, DVC),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

// ── without_declaration ──────────────────────────────────────────────────

#[test]
fn container_augment_before_intro_reports_without_declaration() {
    // The aug lives in an earlier unit than its intro — upstream
    // `precedingFragments` is empty → `augmentation_without_declaration` at
    // the `augment` keyword. The first unit is the reporting one here
    // (the augmentation's own file).
    let main = "augment class B {}\n";
    let augment = "class B {}\n";
    let report = report_ops_augment_intro_experiments(main, augment, &[Feature::Augmentations]);
    assert_eq!(
        rows(&report, WO),
        vec![(at(main, "augment"), 7)],
        "{:?}",
        dump(&report)
    );
}

#[test]
fn same_file_container_augment_over_intro_is_silent() {
    // Oracle: `class B {}` + `augment class B {}` merges — no rows.
    let source = "class B {}\naugment class B {}\n";
    let report = same(source);
    assert_eq!(
        rows(&report, WO),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, KIND),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn enum_constant_augment_reports_constant_variable() {
    // Oracle `ConstantVariableAugmentationTest/test_enum_enumConstant_
    // augmentedByEnumConstant`: `augment foo` inside `augment enum E` —
    // the constant's own `augment` keyword reports
    // `constant_variable_augmentation` at the constant's name (upstream
    // `visitEnumConstantDeclaration`, error_verifier.dart:808-818 — an enum
    // constant fragment is `isConst` unconditionally).
    let source = "enum E {\n  foo\n}\naugment enum E {\n  augment foo,\n}\n";
    let report = same(source);
    assert_eq!(
        rows(&report, CVA),
        vec![(at(source, "augment foo") + 8, 3)],
        "{:?}",
        dump(&report)
    );
}
