
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

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports.
fn unit_report(source: &str) -> FileReport {
    unit_report_at_version(source, LanguageVersion::new(3, 15))
}

/// [`Self::unit_report`] at an explicit settled language version: the
/// unit-level face of the engine's per-file settlement — the `@dart`
/// override lands in the `FeatureSet` before the fold sees it
/// (`scan_unit`'s settled output), so deriving the set from the
/// version here is the same mechanism one level down.
fn unit_report_at_version(source: &str, version: LanguageVersion) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(version);
    let features = ParserFeatures::from_feature_set(set);
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
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
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
        // The same derivation the engine's fold call site makes from
        // the settled set (the `inference-update-2` release gate).
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// The byte offset of `needle`'s first occurrence (ASCII corpus: byte
/// offsets are the row offsets).
fn at(source: &str, needle: &str) -> u32 {
    u32::try_from(
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {source:?}")),
    )
    .expect("offset fits u32")
}

/// Exactly one row of `name`, anchored at `offset`.
fn one_at(report: &FileReport, name: &str, offset: u32, source: &str) {
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "{name}: {rows:?}");
    assert_eq!(rows[0].range.offset, offset, "in {source:?}");
}

// —— Part 1: the function-type-alias boundedness faces ————————————

/// The corpus row (`enum_instantiated_to_bounds_is_not_well_bounded`
/// 13:6): the enum parameter's bound applies a function-type alias to
/// the enum's own parameter and the alias's RHS uses its parameter in
/// the formal position — the infinite-descent shape the official
/// expansion diverges on. The gate reports at the enum name.
#[test]
fn self_referential_alias_bound_reports_not_well_bounded() {
    let source = "typedef A<X> = X Function(X); enum E<T extends A<T>, U> { v<Never, int>() }";
    let report = unit_report(source);
    one_at(
        &report,
        "enum_instantiated_to_bounds_is_not_well_bounded",
        at(source, "E<T"),
        source,
    );
}

/// Well-formed alias instantiations stay silent end-to-end: the
/// expansion pipeline interns the function type without poisoning the
/// alias or reaching the interface arm (the unexpanded head must never
/// descend into the hierarchy as a declaration).
#[test]
fn valid_alias_instantiations_stay_silent() {
    // The `class int {}` stub is the harness's core stand-in (#537): the
    // bare `int` in `A<int>` would otherwise land as a `TypeUse(Missing)`
    // row — an `undefined_class` at pass 2 that crowds the silence pin
    // (the oracle resolves `int` through dart:core).
    let source = "class int {}\ntypedef A<X> = X Function(X); void f(A<int> a) {}";
    let report = unit_report(source);
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset))
            .collect::<Vec<_>>()
    );
}

// —— Part 2a: the const-eval deferred-creation walk ————————————————

/// The corpus rows (`const_eval_type_string` 17:11/17:13): the
/// creation argument's undefined const verdict makes the argument
/// non-constant (`const_with_non_constant_argument` at `x`), the
/// constructor initializer's property access then sees an invalid
/// receiver (`const_eval_type_string` at the whole creation), and the
/// undefined name co-fires — the variable's own face stays silent.
#[test]
fn const_creation_routes_nonstring_via_undefined_argument() {
    let source = "class B {\n  final l;\n  const B(String o) : l = o.length;\n}\n\nconst y = B(x);";
    let report = unit_report(source);
    one_at(
        &report,
        "const_eval_type_string",
        at(source, "B(x)"),
        source,
    );
    one_at(
        &report,
        "const_with_non_constant_argument",
        at(source, "x)"),
        source,
    );
    one_at(&report, "undefined_identifier", at(source, "x)"), source);
}

// —— Parts 2b-2d: the formals' reach into the argument check ————————

/// The unnamed creation's head resolves the constructor's formals:
/// `String` against `int` reports at the argument, once.
#[test]
fn creation_arguments_check_against_declared_formals() {
    let source = "class C { C(int x); } void f() { C('s'); }";
    let report = unit_report(source);
    one_at(
        &report,
        "argument_type_not_assignable",
        at(source, "'s'"),
        source,
    );
}

/// The implicit-this face: an unqualified invocation inside the
/// container resolves the class's own method formals (a same-named
/// local would shadow — none here).
#[test]
fn implicit_this_formals_reach_the_argument_check() {
    let source = "class C { void m(int x) { m('s'); } }";
    let report = unit_report(source);
    one_at(
        &report,
        "argument_type_not_assignable",
        at(source, "'s'"),
        source,
    );
}

/// The declared context solves the generic parameter before the
/// argument check: `g<int>`'s parameter stands for `int`, so `String`
/// reports (the unsubstituted type-parameter face would stay silent).
#[test]
fn declared_context_solves_the_parameter_for_arguments() {
    // `class int {}` follows the harness convention (#537): the solved
    // check target must resolve, or the assignability pair stays
    // undecidable and the row suppresses.
    let source = "class int {} T g<T>(T t) => t; void f() { int x = g<int>('a'); }";
    let report = unit_report(source);
    one_at(
        &report,
        "argument_type_not_assignable",
        at(source, "'a'"),
        source,
    );
}

// —— Part 3: the flow-fact joins ————————————————————————————————————

/// A `bool?` operand in an `if` condition reports
/// `unchecked_use_of_nullable_value` at the operand read, once (the
/// staged candidate is re-decided exactly once at the teardown join).
#[test]
fn nullable_bool_condition_reports_at_the_operand() {
    let source = "void f(bool? b) { if (b) {} }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (b)") + 4,
        source,
    );
}

/// The flow join's headline: a read promoted to non-nullable `bool`
/// (`b != null`'s then branch) keeps the inner condition silent — the
/// parse-time `bool?` candidate is overturned by the flow-aware
/// static type.
#[test]
fn promoted_condition_operand_stays_silent() {
    let source = "void f(bool? b) { if (b != null) { if (b) {} } }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

/// The `!` negation operand is a condition context too.
#[test]
fn negation_operand_is_a_condition() {
    let source = "void f(bool? b) { if (!b) {} }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (!b)") + 5,
        source,
    );
}

/// The lazy operator's left operand is a condition context (the right
/// one is covered by the enclosing junction's re-read; the dedup keeps
/// one row per site).
#[test]
fn lazy_operator_left_operand_is_a_condition() {
    let source = "void f(bool? a, bool b) { if (a && b) {} }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "a && b"),
        source,
    );
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 1);
}

/// `!(x < 0)` is a statically-`bool` parenthesized comparison — the
/// parenthesized row's `inner_ty` reads the comparison's latched `bool`
/// extent (not the last operand row's type, which would be the right
/// operand's `int`), so the `!` operand's negation face stays silent.
/// Regression pin: `clean_members_and_bodies.dart`'s `!(c.m() < 0)`
/// over-fired `non_bool_negation_expression` when the group's row
/// adopted the right operand's `int`.
#[test]
fn parenthesized_comparison_negation_stays_silent() {
    let report = unit_report(
        "class int {} class C { int m() => 1; } \
             void f(C c) { if (!(c.m() < 0)) {} }",
    );
    assert_eq!(
        count_code(&report, "non_bool_negation_expression"),
        0,
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| &d.code.name)
            .collect::<Vec<_>>()
    );
}

/// The matched-type re-decision: `'a'` against the declared `Object`
/// scrutinee passes at parse time (String can equal an Object), but
/// the clause's promoted matched type (`o is int`) can never equal it
/// — the teardown join reports at the constant, once (the parse-time
/// deferral keeps the double emission out).
#[test]
fn promoted_matched_type_redecides_constant_pattern() {
    let source = "void f(Object o) { if (o is int) { switch (o) { case 'a': break; } } }";
    let report = unit_report(source);
    one_at(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "'a'"),
        source,
    );
}

// —— Part 4: the #1404 decision-point pins ————————————————————————

/// A property condition operand stages at the member row and the
/// parse-time `bool?` verdict is final when no promotion stands: flow
/// anchors no condition-type row for property accesses
/// (`Interp::condition_operand` gates on promotable variable reads).
/// The row reports over the whole property-access span — the receiver
/// token through the member's end (`.at(condition)`; capture-proven:
/// the why-not-promoted corpus's 18 condition-face rows all anchor at
/// the property-access start). That is the upstream verdict for a
/// public, non-final field too (`checkForNonBoolExpression`'s
/// `uncheckedUseOfNullableValueAsCondition` fallback,
/// bool_expression_verifier.dart:51-77 — field promotion needs a
/// private, final field): the outer condition's last value row is the
/// null literal's, so only the inner operand stages and exactly one
/// row fires.
#[test]
fn property_condition_operand_reports_at_the_selector() {
    let source = "class C { bool? b; } void f(C c) { if (c.b != null) { if (c.b) {} } }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (c.b) {}") + 4,
        source,
    );
}

/// Record field getters carry the shape's field type (#2139): a
/// `bool?` named field (`r.strict`) and a `bool?` positional field
/// (`p.$1`) in condition position report over the whole
/// property-access span — record fields are synthesized getters, never
/// promotable, so the parse-time verdict is final (probe: `dart
/// analyze` reports `UNCHECKED_USE_OF_NULLABLE_VALUE` at both).
#[test]
fn nullable_record_fields_report_as_conditions() {
    let source = "void f(({bool? strict, int n}) r, (bool?, int) p) { \
                      if (r.strict) {} if (p.$1) {} }";
    let report = unit_report(source);
    let starts: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.range.offset)
        .collect();
    assert_eq!(
        starts,
        [at(source, "r.strict) {}"), at(source, "p.$1) {}")],
        "{source}"
    );
}

/// The typedef face: a record alias folds to its shape before the field
/// read (`typedef R = ({bool? strict}); r.strict`), so the alias use
/// reports exactly like the inline record type.
#[test]
fn aliased_record_field_reports_as_condition() {
    let source = "typedef R = ({bool? strict, int n}); typedef T = (bool?, int); \
                      void f(R r, T t) { if (r.strict) {} if (t.$1) {} }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 2);
}

/// Non-nullable record fields stay silent, and so do the spellings the
/// field face does not own: `$0`, an out-of-range `$3`, a leading-zero
/// `$01`, and a missing named field (unmodeled — under-report).
#[test]
fn non_nullable_and_unowned_record_fields_stay_silent() {
    let source = "void f(({bool strict}) r, (bool, bool?) p) { \
                      if (r.strict) {} if (p.$1) {} if (p.$0) {} if (p.$3) {} \
                      if (p.$01) {} if (r.other) {} }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

// —— #2174: the staged-RHS promotion feed ————————————————————————

/// A plain `=` write whose RHS the rows resolved (`st(start)` — an
/// in-file static the flow walk types as unknown) promotes the target:
/// the semantic stages the resolved RHS type keyed on the target token,
/// and the fold adopts it when its own answer is unknown. The read
/// after the write answers the promoted type, so the argument row
/// suppresses (`int? end; end = st(0); g(1, 2, end);` is clean).
#[test]
fn staged_rhs_promotes_the_write_target() {
    let source = "int st(int x) => x;\n\
                      void g(int a, int b, int c) {}\n\
                      void f([int start = 0, int? end]) {\n  \
                      end = st(start);\n  \
                      g(1, 2, end);\n\
                      }";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        0,
        "{report:?}"
    );
}

/// The compound write keeps the promoted read type (`initialCapacity
/// += 1` after a null-check early return): the operator result at name
/// level is the promoted operand, so the read-modify-write never
/// demotes what the read had promoted.
#[test]
fn compound_write_keeps_the_promoted_read() {
    let source = "void g(int x) {}\n\
                      void f(int? n) {\n  \
                      if (n == null) return;\n  \
                      n += 1;\n  \
                      g(n);\n\
                      }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "argument_type_not_assignable"), 0);
}

/// The signature-less function head promotes through `??=`: a nullable
/// function-typed formal's declared head carries its written
/// nullability, so `compare ??= defaultCompare` leaves the argument
/// read promoted (the argument row stages against the real declared
/// type with its `?` stripped and re-decides).
#[test]
fn nullable_function_typed_formal_promotes_through_if_null_assign() {
    let source = "int defaultCompare() => 0;\n\
                      void g(int Function() compare) {}\n\
                      void f({int Function()? compare}) {\n  \
                      compare ??= defaultCompare;\n  \
                      g(compare);\n\
                      }";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        0,
        "{report:?}"
    );
}

/// The staged write's promoted read feeds inference: a `var`-inferred
/// declarator whose initializer is a bare read of the staged target
/// infers the staged type (the oracle's flow-aware inference), so the
/// loop-head join demotes to `int`, not `int?` — the condition read and
/// every in-loop use stay clean (the corpus face:
/// `binarySearchBy`'s `var max = end` after the resolved-RHS write).
#[test]
fn inferred_local_takes_the_staged_write_type_through_the_loop() {
    let source = "int check(int a, int? b, int c) => 0;\n\
                      void f([int start = 0, int? end]) {\n  \
                      end = check(start, end, 0);\n  \
                      var max = end;\n  \
                      while (0 < max) {\n    \
                      var mid = max - 1;\n    \
                      max = mid;\n  \
                      }\n\
                      }";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        0,
        "{report:?}"
    );
}

/// A frame local shadows a same-spelled top-level function in the
/// argument tear-off face: the read is the local's value, never the
/// function's signature-as-a-value (the scope precedence
/// `resolve_bare` applies; the corpus face: path's
/// `setExtension(String path, String extension)` over the top-level
/// `String extension(String, [int])`).
#[test]
fn local_formal_shadows_the_top_level_tear_off_in_arguments() {
    let source = "class Ctx {\n  \
                      String setExtension(String path, String extension) => '';\n\
                      }\n\
                      Ctx get context => Ctx();\n\
                      String extension(String path, [int level = 1]) => '';\n\
                      String setExtension(String path, String extension) =>\n  \
                      context.setExtension(path, extension);";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        0,
        "{report:?}"
    );
}

// —— #2140: the core-type nullable-dereference face ——————————————————

/// A member the core table defines, read through a merely-nullable
/// receiver, reports `unchecked_use_of_nullable_value` at the selector
/// (nullable_dereference_verifier.dart report — the found-arm the
/// walkable-model face already makes, extended to the
/// external-primitive face; probe: `int f(String? s) { return
/// s.length; }` reports at `length`). Declared and inferred
/// (`final x = e`) locals share the receiver face.
#[test]
fn core_type_nullable_receiver_dereference_reports() {
    let source = "int f(String? s) { return s.length; }";
    one_at(
        &unit_report(source),
        "unchecked_use_of_nullable_value",
        at(source, "length"),
        source,
    );
    let inferred = "int f(String? s) { final alias = s; return alias.length; }";
    one_at(
        &unit_report(inferred),
        "unchecked_use_of_nullable_value",
        at(inferred, "length"),
        inferred,
    );
}

/// The corpus fixture row (sem2 promotion_patterns_flow_records.dart
/// 70:18): the `case null` branch's dereference reports at the
/// selector while the fall-through read below stays clean — the same
/// pattern demotes inside the branch and promotes `alias` to `String`
/// on the fall-through, so the promotion machinery (the flow layer's
/// declared-type adoption, #2140) silences the second read.
#[test]
fn case_null_branch_dereference_reports_and_fall_through_stays_clean() {
    let source = "int f(({String? alias}) r) { final alias = r.alias; \
                      if (alias case null) { return alias.length; } \
                      return alias.length; }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 1);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "length"),
        source,
    );
}

/// The promotion silences: an `!= null`-guarded read and a `?.` chain
/// stay clean over an inferred local, exactly like an annotated one.
#[test]
fn promoted_and_null_aware_inferred_local_reads_stay_clean() {
    let guarded = "int f(String? s) { final alias = s; \
                       if (alias != null) { return alias.length; } return 0; }";
    assert_eq!(
        count_code(&unit_report(guarded), "unchecked_use_of_nullable_value"),
        0,
        "{guarded}"
    );
    let null_aware = "int f(String? s) { final alias = s; return alias?.length ?? 0; }";
    assert_eq!(
        count_code(&unit_report(null_aware), "unchecked_use_of_nullable_value"),
        0,
        "{null_aware}"
    );
}

/// The dereference question never reaches the Object members
/// (`s.hashCode`, `s.toString()`, `s.runtimeType` — the oracle's
/// property resolution answers them without a dereference check;
/// probe-verified silent), and a non-nullable receiver stays clean.
#[test]
fn object_members_and_non_nullable_receivers_stay_clean() {
    let source = "void f(String? s, String t) { \
                      s.hashCode; s.runtimeType; s.toString(); t.length; }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

/// The write face: a core-settable member (`List`'s `length=`) through
/// a nullable receiver reports the same code at the selector (probe:
/// `void f(List? l) { l.length = 2; }` — the property message).
#[test]
fn core_type_nullable_receiver_write_reports() {
    let source = "void f(List? l) { l.length = 2; }";
    one_at(
        &unit_report(source),
        "unchecked_use_of_nullable_value",
        at(source, "length"),
        source,
    );
}

/// The named creation's selector resolves the constructor's formals
/// through the receiver fallback (`ctor_formals` keys the selector —
/// the collector's both-lookup-spellings contract): `String` against
/// `int` reports at the argument, once.
#[test]
fn named_creation_arguments_check() {
    let source = "class C { C.named(int x); } void f() { C.named('s'); }";
    let report = unit_report(source);
    one_at(
        &report,
        "argument_type_not_assignable",
        at(source, "'s'"),
        source,
    );
}

/// The collection if-element's condition is a condition context: the
/// staging anchor rides `handle_then_control_flow` (the parser fires
/// it right after the parenthesized condition — the official
/// `handle_ifElement_conditionEnd` position, upstream Dart SDK
/// source, `pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart:
/// 1153-1166`), so `[if (b) 1]` reports at the operand like the
/// statement form.
#[test]
fn if_element_condition_is_a_condition_context() {
    let source = "void f(bool? b) { var x = [if (b) 1]; }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (b)") + 4,
        source,
    );
}

/// The remaining condition heads of the family (each mirrors the
/// `if`-statement pin): a `bool?`-typed condition operand reports at
/// the operand, once per head — the while/do-while anchors at the
/// body/`while` events, the conditional's at the `?`, the junction's
/// ws-adjacent left arm, the assert and C-`for` within-span lookups.
#[test]
fn while_condition_operand_reports() {
    let source = "void f(bool? b) { while (b) {} }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "while (b)") + 7,
        source,
    );
}

#[test]
fn do_while_condition_operand_reports() {
    let source = "void f(bool? b) { do {} while (b); }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "while (b)") + 7,
        source,
    );
}

#[test]
fn conditional_expression_condition_operand_reports() {
    let source = "void f(bool? b) { var x = b ? 1 : 2; }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "b ? 1 : 2"),
        source,
    );
}

/// The `||` junction's left operand is a condition context like the
/// `&&` one (the right one is covered by the enclosing head's
/// re-read; the dedup keeps one row per site).
#[test]
fn lazy_or_left_operand_is_a_condition() {
    let source = "void f(bool? a, bool b) { if (a || b) {} }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "a || b"),
        source,
    );
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 1);
}

#[test]
fn assert_condition_operand_reports() {
    let source = "void f(bool? b) { assert(b); }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "assert(b)") + 7,
        source,
    );
}

#[test]
fn for_condition_operand_reports() {
    let source = "void f(bool? b) { for (;b;) {} }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "for (;b;)") + 6,
        source,
    );
}

/// The shadow guard's negative (`check_invocation_arity`'s bare
/// face): a same-named local function suppresses the container's
/// member formals, and the local's own shape answers dynamically
/// (the name level carries no formal table for local functions; no
/// top-level declaration answers either) — the member's `int` never
/// fires against the `String` argument. Suppression is total: no
/// formals win.
#[test]
fn shadowed_local_function_suppresses_member_formals() {
    let source = "class C { void m(int x) {} void f() { void m(String s) {} m('s'); } }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "argument_type_not_assignable"), 0);
}

/// The int/double cross answers "possible"
/// (`constant_never_matches`'s primitive-equality guard — the
/// official's JavaScript note): an int constant against a `double`
/// scrutinee stays silent.
#[test]
fn int_constant_pattern_matches_double_scrutinee() {
    let source = "void f(double d) { switch (d) { case 1: } }";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "constant_pattern_never_matches_value_type"),
        0
    );
}

/// The creation's downward face: the declared context's written
/// arguments name the container's parameters (`A<int>` solves `T` to
/// `int` before the argument check — the solver's downward
/// inference), so `String` reports.
#[test]
fn creation_context_solves_the_parameter_for_arguments() {
    // The `class int {}` stub is the harness's core stand-in (#537):
    // the `int` in `A<int>` would otherwise land as a `TypeUse(Missing)`
    // row — a `non_type_as_type_argument` at pass 2 that crowds the
    // solve pin.
    let source = "class int {} class A<T> { A(T t); } void f() { A<int> a = A('s'); }";
    let report = unit_report(source);
    one_at(
        &report,
        "argument_type_not_assignable",
        at(source, "'s'"),
        source,
    );
}

/// The bound clamp: a `null` argument solves `T extends num` to
/// `Null`, the solution violates the bound, and the check runs
/// against the bound-respecting solution — the row names `num`.
#[test]
fn bound_violating_solution_clamps_to_the_bound() {
    let source = "T f<T extends num>(T t) => t; void g() { f(null); }";
    let report = unit_report(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    // The oracle keeps the template's trailing space — the `{2}` record
    // appendix substitutes empty but the literal space before it stays
    // (upstream Dart SDK source, pkg/analyzer/lib/src/diagnostic/
    // diagnostic.g.dart:641-651; 234/234 capture rows end `". "`).
    assert_eq!(
        rows[0].message,
        "The argument type 'Null' can't be assigned to the parameter type 'num'. "
    );
}

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name.as_ref())
        .collect()
}

// —— Field promotion pins (#1409). Each pin cites its upstream
// verdict; eligibility per upstream Dart SDK source,
// pkg/_fe_analyzer_shared/lib/src/field_promotability.dart
// :70-76 (a field is promotable iff final + private name + no
// conflicting declaration), the machinery per
// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart.

/// Pin 1: a private final field tested non-null silences the
/// downstream use — the promotion row (anchored at the member name)
/// overturns the parse-time `bool?` condition candidate (the official
/// `_handleProperty` promotion lookup, `flow_analysis.dart:9010-9054`).
#[test]
fn pin1_private_final_field_tested_then_used_stays_silent() {
    let source = "class C { final bool? _b; }\nvoid f(C c) { if (c._b != null) { if (c._b) {} } }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

/// Pin 2: a public final field is not promotable (the name lacks the
/// `_` prefix, `field_promotability.dart:70-76`) — the identical shape
/// still reports at the use, anchored at the property-access start
/// (`.at(condition)` — capture-proven: the why-not-promoted corpus's
/// condition-face rows all anchor at the receiver token).
#[test]
fn pin2_public_final_field_still_fires() {
    let source = "class C { final bool? b; }\nvoid f(C c) { if (c.b != null) { if (c.b) {} } }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (c.b) {}") + 4,
        source,
    );
}

/// Pin 3: a private non-final field is not promotable (finality is
/// required, `field_promotability.dart:70-76`) — the use still fires,
/// at the property-access start.
#[test]
fn pin3_private_non_final_field_still_fires() {
    let source = "class C { bool? _b; }\nvoid f(C c) { if (c._b != null) { if (c._b) {} } }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (c._b) {}") + 4,
        source,
    );
}

/// Pin 4: the promotion keys on the receiver — another receiver's read
/// of the same member is untested and still fires (receiver-keyed
/// grant; the anchor stays the property-access start).
#[test]
fn pin4_cross_receiver_use_still_fires() {
    let source =
        "class C { final bool? _b; }\nvoid f(C c1, C c2) { if (c1._b != null) { if (c2._b) {} } }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (c2._b) {}") + 4,
        source,
    );
}

/// Pin 5: a single-arm promotion dies at the end-if join (the official
/// joinPromotionInfo common-prefix rule,
/// `flow_analysis.dart:5830-5940`) — the use after the join fires, at
/// the property-access start.
#[test]
fn pin5_single_arm_promotion_after_join_fires() {
    let source = "class C { final bool? _b; }\nvoid f(C c, bool x) { if (x) { if (c._b != null) {} } if (c._b) {} }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (c._b) {}") + 4,
        source,
    );
}

/// Pin 6: `== null` and the negated test take the inversion face (the
/// property is non-null on the FALSE arm only) — the then-branch use
/// fires, at the property-access start (the negation shape anchors at
/// the OPERAND start, upstream `.at(operand)`).
#[test]
fn pin6_inverse_tests_do_not_promote() {
    let eq_null = "class C { final bool? _b; }\nvoid f(C c) { if (c._b == null) { if (c._b) {} } }";
    let report = unit_report(eq_null);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(eq_null, "if (c._b) {}") + 4,
        eq_null,
    );
    let negated =
        "class C { final bool? _b; }\nvoid f(C c) { if (!(c._b != null)) { if (c._b) {} } }";
    let report = unit_report(negated);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(negated, "if (c._b) {}") + 4,
        negated,
    );
}

/// Pin 7: `this._b` and the implicit-`this` bare `_b` both promote
/// (the official `ThisPropertyTarget` for implicit members and
/// `thisExpression()` for explicit `this`,
/// `flow_analysis.dart:5977-5990`/:8149-8153) — both shapes stay
/// silent.
#[test]
fn pin7_this_and_implicit_this_promote() {
    let explicit =
        "class C { final bool? _b; void m() { if (this._b != null) { if (this._b) {} } } }";
    let report = unit_report(explicit);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
    let implicit = "class C { final bool? _b; void m() { if (_b != null) { if (_b) {} } } }";
    let report = unit_report(implicit);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

/// Pin 8: `late final` private fields promote — the promotability
/// verdict never consults `late` (`field_promotability.dart:70-76`;
/// the official filter rejects external/non-final fields and
/// conflicting declarations only, :100-172) — the shape stays silent.
#[test]
fn pin8_late_final_private_field_promotes() {
    let source =
        "class C { late final bool? _b; }\nvoid f(C c) { if (c._b != null) { if (c._b) {} } }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

/// Pin 9: a library settled below 3.2 (`inference-update-2`'s
/// `enabledIn`, `experiments.g.dart:499`) gets NO property
/// promotion — the version grant is the *accessing* library's
/// feature set (`type_analyzer_options.dart:17` via
/// `ast_resolver.dart:139-140`), and the identical promotable shape
/// still reports `unchecked_use_of_nullable_value` at the
/// property-access start.
#[test]
fn pin9_pre_3_2_library_still_fires() {
    let source = "class C { final bool? _b; }\nvoid f(C c) { if (c._b != null) { if (c._b) {} } }";
    let report = unit_report_at_version(source, LanguageVersion::new(3, 1));
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (c._b) {}") + 4,
        source,
    );
}

/// Pin 10: the boundary itself — at exactly 3.2 the grant turns on
/// and the identical shape stays silent (the released feature is
/// enabled iff the settled version ≥ its release version).
#[test]
fn pin10_at_3_2_library_stays_silent() {
    let source = "class C { final bool? _b; }\nvoid f(C c) { if (c._b != null) { if (c._b) {} } }";
    let report = unit_report_at_version(source, LanguageVersion::new(3, 2));
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

/// The byte-invariance guard, diagnostic-level: local-variable
/// promotion behavior is unchanged alongside property reads — the
/// interleaved property read neither silences the local's own
/// promoted use nor adds a row.
#[test]
fn local_promotion_unchanged_alongside_property_reads() {
    let source =
        "class C { final bool? _b; }\nvoid f(C c, bool? b) { if (b != null) { c._b; if (b) {} } }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

/// Pin 11: the catch entry joins models AND property slots (official
/// `catchBegin`'s discardPromotionsAndMarkNotUnassigned,
/// flow_analysis.dart:4748) — a receiver written in the try body kills
/// the pre-try promotion on the catch path, and the use fires.
#[test]
fn pin11_catch_entry_receiver_write_still_fires() {
    let source = "class C { final bool? _b; }\nvoid f(C c) { if (c._b != null) { try { c = C(); } catch (e) { if (c._b) {} } } }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (c._b) {}") + 4,
        source,
    );
}

/// Pin 12: the switch's labeled-case entry joins models AND property
/// slots — a receiver written in any case body kills the carried
/// promotion on the labeled jump-target path, and the use fires.
#[test]
fn pin12_labeled_case_entry_receiver_write_still_fires() {
    let source = "class C { final bool? _b; }\nvoid f(C c, int x) { if (c._b != null) { switch (x) { L: case 1: if (c._b) {} break; case 2: c = C(); break; } } }";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        at(source, "if (c._b) {}") + 4,
        source,
    );
}

/// Pin 13: a same-name extension getter does not poison the field —
/// the promotability pass registers classes, enums, and mixins only
/// (upstream Dart SDK source,
/// pkg/analyzer/lib/src/summary2/library_builder.dart:885-905), and
/// the getter is dead at every read site (the instance member wins) —
/// so the promotion applies.
#[test]
fn pin13_extension_getter_does_not_poison() {
    let source = "class C { final bool? _b; }\nextension E on C { int get _b => 0; }\nvoid f(C c) { if (c._b != null) { if (c._b) {} } }";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "unchecked_use_of_nullable_value"), 0);
}

/// #1758 probe: an old-style function-typed formal's name binds to the
/// parameter — reads/invocations resolve, never `undefined_*`.
#[test]
fn function_typed_formal_name_binds() {
    // The `class int {}` stub is the harness's core stand-in (#537).
    let report = unit_report("class int {}\nvoid g(int f()) { f(); f; }");
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset))
            .collect::<Vec<_>>()
    );
}

/// #1758: the function-typed formal's declared type is its function
/// type (`int f()` → `int Function()`), so a null-aware receiver check
/// judges the function type's own nullability — `?.` on the non-null
/// `int f()` is `invalid_null_aware_operator`, while the same access on
/// `int? f()` is legal and emits nothing.
#[test]
fn function_typed_formal_type_binds() {
    // The `int` stub declares `isEven` so the member face resolves on
    // the call's return — the pin isolates the `?.` verdict (the
    // oracle's int carries isEven; a memberless stub would add the
    // collateral `undefined_getter` rows).
    let source = "class bool {} class int { bool get isEven => throw 0; }\nvoid m(int f()) { f()?.isEven; }\nvoid n(int? f()) { f()?.isEven; }";
    let report = unit_report(source);
    let names = report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset))
        .collect::<Vec<_>>();
    // One row: `?.` on `m`'s non-null formal, anchored at the operator.
    let op = source.find("?.").unwrap() as u32;
    assert_eq!(names, vec![("invalid_null_aware_operator", op)]);
}

/// #1758: the same verdict on the index and cascade faces — `f()?[i]`,
/// `f()?..[i]` and `f()?..m()` read the send's return type through the
/// same slot.
#[test]
fn function_typed_formal_index_and_cascade() {
    let source = "class int {}\nclass List<T> { T operator [](int i) => throw 0; T get first => throw 0; }\nvoid a2(List<int> f()) { f()?.first(throw ''); }\nvoid b2(List<void Function(dynamic)> f()) { f()?.first(throw ''); }\nvoid c2(List<int> f()) { f()?..[throw '']; }\nvoid d2(List<int> f()) { f()?..first(throw ''); }";
    let report = unit_report(source);
    let names = report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset))
        .collect::<Vec<_>>();
    // All four receivers are non-nullable `f()` sends — every null-aware
    // operator reports, anchored at its `?` (the four statement bodies'
    // `?` operators are the only `?` tokens after the stub decls).
    let positions: Vec<u32> = source.match_indices("?.").map(|(i, _)| i as u32).collect();
    assert_eq!(
        names,
        positions
            .iter()
            .map(|o| ("invalid_null_aware_operator", *o))
            .collect::<Vec<_>>()
    );
}

/// #1534 F-H regression pin: a member access sitting as the right
/// operand of `??`/`==`/`!=`/`&&`/`||` inside an open send's argument
/// window still resolves through the member face — the five
/// `undefined_getter 'bad'` rows of
/// `why_not_promoted/argument_type_not_assignable_nullability_error.dart`
/// (132:14, 147:14, 148:14, 168:91, 189:91) were the displacement: the
/// access merged against the callee placeholder instead of its
/// receiver. The oracle's own row (`argument_type_not_assignable` on
/// the operand, `.at(expression)`) must stand with no getter miss.
#[test]
fn member_access_inside_send_argument_operands_resolves() {
    let source = "class int {}\nclass bool {}\n\
                      class C { int? bad; void f(int i) {} }\n\
                      void g(C c, int? i, bool b) {\n\
                      if (c.bad == null) return;\n\
                      c.f(i ?? c.bad);\n\
                      c.f(b == c.bad);\n\
                      c.f(b != c.bad);\n\
                      c.f(b && c.bad == null);\n\
                      }";
    let report = unit_report(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect();
    // No receiver-resolution miss anywhere: every `c.bad` binds.
    assert!(
        !rows.iter().any(|(name, _, _)| *name == "undefined_getter"),
        "{rows:?}"
    );
}

/// `final_not_initialized_constructor` on the extension-type
/// representation field: every declared constructor that leaves the
/// representation unbound reports at its own head (error_verifier.dart
/// drives `ConstructorFieldsVerifier.addConstructors` for extension
/// types like classes).
#[test]
fn extension_type_representation_reports_per_constructor() {
    let report = unit_report(
        "class int {} extension type ET(int i) {\n  new.named();\n  factory.named();\n}",
    );
    assert_eq!(
        count_code(&report, "final_not_initialized_constructor"),
        2,
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        count_code(&report, "duplicate_constructor"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// `new_with_undefined_constructor_default` on the unnamed miss: a
/// class whose every constructor is named (`factory H` inside
/// `class G`) leaves the unnamed face absent, and `G()` reports at the
/// creation head.
#[test]
fn unnamed_creation_miss_reports_default_code() {
    let report = unit_report("class G {\n  factory H() => G();\n}");
    assert_eq!(
        count_code(&report, "new_with_undefined_constructor_default"),
        1,
        "{:?}",
        code_names(&report)
    );
    // A class with only the implicit default stays silent.
    let implicit = unit_report("class A {}\nvoid f() {\n  var a = A();\n  print(a);\n}");
    assert_eq!(
        count_code(&implicit, "new_with_undefined_constructor_default"),
        0,
        "{:?}",
        code_names(&implicit)
    );
}

/// `factory new()` (the FACTORY_CONSTRUCTOR_NEW_NAME recovery head):
/// the bare-`new` spelling IS the unnamed constructor's name, so `K()`
/// in the body resolves and the unnamed-miss code stays silent — while
/// the dotted `new.named()` shape keeps the unnamed face absent (the
/// oracle reports the miss there).
#[test]
fn factory_new_head_serves_the_unnamed_face() {
    let bare = unit_report("class K {\n  factory new() => K();\n}");
    assert_eq!(
        count_code(&bare, "new_with_undefined_constructor_default"),
        0,
        "{:?}",
        code_names(&bare)
    );
    let dotted = unit_report("class K {\n  factory new.named() => K();\n}");
    assert_eq!(
        count_code(&dotted, "new_with_undefined_constructor_default"),
        1,
        "{:?}",
        code_names(&dotted)
    );
}

/// `duplicate_constructor` never crosses same-named class declarations
/// (#2138): each `class C(...)` declaration owns its constructor set —
/// the oracle reports `duplicate_definition` on the repeated class name
/// and no constructor-level row — while the same declaration's second
/// unnamed constructor still reports.
#[test]
fn duplicate_constructor_never_crosses_declarations() {
    let report = unit_report(
        "class C(var this.str);\nclass C(final this.str);\nclass C(const this.str);\nclass D(var super.str);\nclass D(final super.str);\nclass D(const super.str);",
    );
    assert_eq!(
        count_code(&report, "duplicate_constructor"),
        0,
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        count_code(&report, "duplicate_definition"),
        4,
        "{:?}",
        code_names(&report)
    );
    // Same-declaration repeats keep the constructor-level row.
    let same = unit_report("class A {\n  A();\n  A();\n}");
    assert_eq!(
        count_code(&same, "duplicate_constructor"),
        1,
        "{:?}",
        code_names(&same)
    );
}

/// An unresolved identifier in a const creation's argument list
/// (#2138): the argument face reports (`const_with_non_constant_argument`
/// — the oracle's `dynamic` substitution) while the declaration's own
/// `const_initialized_with_non_constant_value` stays silent
/// (`hasKnownValue` false). A bare unresolved initializer and a defined
/// non-const argument still report at their spans.
#[test]
fn unresolved_const_argument_stays_declaration_silent() {
    let report = unit_report(
        "var v = 0;\nclass B {\n  final l;\n  const B(String o) : l = o.length;\n}\nconst a = x;\nconst y = B(x);\nconst w = B(v);",
    );
    let rows: Vec<&str> = code_names(&report);
    assert_eq!(
        count_code(&report, "const_initialized_with_non_constant_value"),
        2,
        "{rows:?}"
    );
    assert_eq!(
        count_code(&report, "const_with_non_constant_argument"),
        2,
        "{rows:?}"
    );
    assert_eq!(count_code(&report, "undefined_identifier"), 2, "{rows:?}");
}

/// `implicit_this_reference_in_initializer`: an instance-member
/// reference inside an instance field's initializer reports at the
/// referencing expression — the enum's synthesized `index` getter is
/// the corpus's implicit shape; a static field's initializer stays
/// silent and a method body's reads never reach the face.
#[test]
fn instance_member_in_field_initializer_reports() {
    let report = unit_report("class int {} enum G {\n  a,\n  b;\n\n  final int code = index;\n}");
    assert_eq!(
        count_code(&report, "implicit_this_reference_in_initializer"),
        1,
        "{:?}",
        code_names(&report)
    );
    let method_body =
        unit_report("class int {} class C {\n  int m() => 1;\n\n  final int x = 0;\n}");
    assert_eq!(
        count_code(&method_body, "implicit_this_reference_in_initializer"),
        0,
        "{:?}",
        code_names(&method_body)
    );
}

/// `not_a_type` on an extension-shadowed name: a type position sharing
/// an extension's name resolves to the extension element
/// (`_lookupGetter`'s sole getter-namespace entry) and reports — the
/// built-in `Function` type included.
#[test]
fn extension_name_in_type_position_is_not_a_type() {
    let report = unit_report(
        "class Function {}\ntypedef Function = int;\nextension Function on List {}\nmixin Function {}\nvoid main() {\n  Function ok;\n}",
    );
    let rows: Vec<&str> = code_names(&report);
    assert!(rows.contains(&"not_a_type"), "{rows:?}");
    // A plain extension name in a type position reports too.
    let plain = unit_report("extension E on int {}\nvoid f() {\n  E x;\n  print(x);\n}");
    assert_eq!(
        count_code(&plain, "not_a_type"),
        1,
        "{:?}",
        code_names(&plain)
    );
}

/// The type-argument face is the DIRECT parent (`node.parent is
/// TypeArgumentList`): a function/record type nested in a type argument
/// reports its own return/formal/field misses as plain
/// `undefined_class`, and a type argument under an unresolvable head
/// still reports (oracle-probed: `W<L>` → `undefined_class` on `W` and
/// `non_type_as_type_argument` on `L`).
#[test]
fn type_argument_face_is_the_direct_parent_and_survives_a_dead_head() {
    let codes = |source: &str| {
        let report = unit_report(source);
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset))
            .filter(|(c, _)| *c == "undefined_class" || *c == "non_type_as_type_argument")
            .collect::<Vec<_>>()
    };
    // `List<L Function()> c;` — `List` is a core name the rig lacks, so
    // pin on the inner `L` (offset 5) only.
    let nested = codes("List<L Function(L)> c;");
    assert!(nested.contains(&("undefined_class", 5)), "{nested:?}");
    assert!(nested.contains(&("undefined_class", 16)), "{nested:?}");
    assert!(
        !nested
            .iter()
            .any(|&(c, o)| c == "non_type_as_type_argument" && (o == 5 || o == 16)),
        "{nested:?}"
    );
    let dead = codes("W<L> a;");
    assert!(dead.contains(&("undefined_class", 0)), "{dead:?}");
    assert!(dead.contains(&("non_type_as_type_argument", 2)), "{dead:?}");
}

/// `non_type_as_type_argument` for the recovered old-style function
/// typedef: the return type's type arguments resolve beside their dead
/// head (`typedef e = foo<F>(int x)` — the oracle reports
/// undefined_class on `foo` and non_type_as_type_argument on `F`, six
/// rows over the corpus's two typedefs).
#[test]
fn type_arguments_resolve_beside_a_dead_head() {
    let report = unit_report("typedef e = foo<F>(int x);\ntypedef g = foo<F, G, H, I, J>(int x);");
    assert_eq!(
        count_code(&report, "non_type_as_type_argument"),
        6,
        "{:?}",
        code_names(&report)
    );
}

/// The const-declaration creation faces: `const x = C()` over a
/// non-const constructor reports `const_with_non_const` at the
/// creation, `const z = C(n)` over a non-constant argument reports
/// `const_initialized_with_non_constant_value` at the argument, and
/// `const y = C.named()` over a missing selector reports the
/// initializer row without the `const_with_non_const` twin.
#[test]
fn const_declaration_creations_report_per_verdict() {
    let non_const = unit_report("class C {\n  C();\n}\n\nconst x = C();");
    assert_eq!(
        count_code(&non_const, "const_with_non_const"),
        1,
        "{:?}",
        code_names(&non_const)
    );
    assert_eq!(
        count_code(&non_const, "const_initialized_with_non_constant_value"),
        1,
        "{:?}",
        code_names(&non_const)
    );
    let arg = unit_report(
        "class int {} class C {\n  final int x;\n  const C(this.x);\n}\n\nvar n = 1;\n\nconst z = C(n);",
    );
    assert_eq!(
        count_code(&arg, "const_initialized_with_non_constant_value"),
        1,
        "{:?}",
        code_names(&arg)
    );
    let undefined = unit_report("class C {\n  const C();\n}\n\nconst y = C.named();");
    // The unresolved selector keeps the resolution row and fails the
    // initializer at the whole creation (the oracle's
    // const_with_undefined_constructor fixture emits both).
    assert_eq!(
        count_code(&undefined, "undefined_method"),
        1,
        "{:?}",
        code_names(&undefined)
    );
    assert_eq!(
        count_code(&undefined, "const_initialized_with_non_constant_value"),
        1,
        "{:?}",
        code_names(&undefined)
    );
    assert_eq!(
        count_code(&undefined, "const_with_non_const"),
        0,
        "{:?}",
        code_names(&undefined)
    );
    // A forward const constructor stays silent (the whole-file verdict).
    let forward = unit_report("const x = C();\n\nclass C {\n  const C();\n}");
    assert_eq!(
        count_code(&forward, "const_with_non_const"),
        0,
        "{:?}",
        code_names(&forward)
    );
}

/// `list_element_type_not_assignable`: a typed list literal's element
/// checks against the written element type at the failing element.
#[test]
fn typed_list_literal_checks_element_type() {
    let report = unit_report(
        "class int {} class String {} void main() {\n  var l = <int>['a'];\n  print(l);\n}",
    );
    assert_eq!(
        count_code(&report, "list_element_type_not_assignable"),
        1,
        "{:?}",
        code_names(&report)
    );
    let clean = unit_report(
        "class int {} class String {} void main() {\n  var l = <int>['a', 1];\n  print(l);\n}",
    );
    assert_eq!(
        count_code(&clean, "list_element_type_not_assignable"),
        1,
        "{:?}",
        code_names(&clean)
    );
}

/// `extension_override_argument_not_assignable` with a generic on-type:
/// the override argument checks against the written `on List<Book>`
/// face, not the erasure head.
#[test]
fn extension_override_checks_generic_on_type() {
    let report = unit_report(
        "class int {} class Book {}\nclass List<T> {}\nclass String {}\nextension CatalogExt on List<Book> {\n  List<String> shelfMarks() => const [];\n}\n\nvoid overrideFace(List<int> numbers) {\n  print(CatalogExt(numbers).shelfMarks());\n}",
    );
    assert_eq!(
        count_code(&report, "extension_override_argument_not_assignable"),
        1,
        "{:?}",
        code_names(&report)
    );
}

/// The nullable-receiver property face: an interface member reached
/// through a merely-nullable receiver reports the property variant at
/// the selector; a nullable-on extension member never dereferences.
#[test]
fn nullable_receiver_property_access_reports() {
    let report = unit_report(
        "class int {} class String {\n  int get length => 0;\n}\nclass C {\n  String? s;\n  int f() {\n    return s.length;\n  }\n}",
    );
    assert_eq!(
        count_code(&report, "unchecked_use_of_nullable_value"),
        1,
        "{:?}",
        code_names(&report)
    );
}

/// #2144: a new-style function-typed formal's invocation reads its
/// shape's return type — `bool Function(E) test` makes `if (test(e))`
/// a `bool` condition. Without the `var_types`→`Ty::Function` arm in
/// `callee_return_ty` the send's return stayed `Unknown` and
/// `non_bool_condition` over-fired (events.dart's collection-`if`).
#[test]
fn function_typed_formal_new_style_return_resolves() {
    let report = unit_report(
        "class bool {} class E {}\nclass List<T> { void add(T t) {} }\nclass C {\n  List<E> pick(bool Function(E) test) => [for (final e in _src()) if (test(e)) e];\n  List<E> _src() => [];\n}",
    );
    assert_eq!(
        count_code(&report, "non_bool_condition"),
        0,
        "{:?}",
        code_names(&report)
    );
    // The negative control: an `int`-returning formal still fires.
    let bad = unit_report(
        "class bool {} class int {} class E {}\nvoid f(int Function(E) test, E e) {\n  if (test(e)) {}\n}",
    );
    assert_eq!(
        count_code(&bad, "non_bool_condition"),
        1,
        "{:?}",
        code_names(&bad)
    );
}

/// #2144: a typed list literal folds to one row — the `List<int> x =
/// const <num>[...]` declaration initializer asks its assignability
/// question of the literal's `List<num>` and reports
/// `invalid_assignment` (broken_argument_types.dart). The element rows
/// drain to pass 2 like the send fold's arguments; without the drain
/// the `init_mark` single-row window never saw the literal.
#[test]
fn const_typed_list_initializer_reports_invalid_assignment() {
    let report = unit_report(
        "class int {} class num {} class List<T> {}\nvoid f() {\n  List<int> sizes = const <num>[1, 2, 3];\n  print(sizes);\n}",
    );
    assert_eq!(
        count_code(&report, "invalid_assignment"),
        1,
        "{:?}",
        code_names(&report)
    );
    // The non-const spelling folds the same way.
    let plain = unit_report(
        "class int {} class num {} class List<T> {}\nvoid f() {\n  List<int> sizes = <num>[1, 2, 3];\n  print(sizes);\n}",
    );
    assert_eq!(
        count_code(&plain, "invalid_assignment"),
        1,
        "{:?}",
        code_names(&plain)
    );
    // Matching element type stays silent.
    let ok = unit_report(
        "class int {} class num {} class List<T> {}\nvoid f() {\n  List<num> sizes = const <num>[1, 2, 3];\n  print(sizes);\n}",
    );
    assert_eq!(
        count_code(&ok, "invalid_assignment"),
        0,
        "{:?}",
        code_names(&ok)
    );
}

/// The unannotated collection literal's return face
/// (`return_of_invalid_type`, 3.13.1 probes): the literal's row carries
/// no type of its own, so the host's declared return resolves it —
/// `int f() => []` / `int f() => {}` / `int f() => <int>{}` all reject
/// (the collection heads are final classes disjoint from the final
/// primitives), while a `List<int>` host adopts the literal and stays
/// silent.
#[test]
fn untyped_collection_literal_return_checks() {
    let harness = "class int {} class List<T> {} class Set<T> {} class Map<K, V> {}\n";
    for body in ["int f() => [];", "int f() => {};", "int f() => <int>{};"] {
        let report = unit_report(&format!("{harness}{body}"));
        assert_eq!(
            count_code(&report, "return_of_invalid_type"),
            1,
            "{body}: {:?}",
            code_names(&report)
        );
    }
    // The context-typed resolutions stay silent: a `List` host adopts
    // the `[]` literal, an empty `{}` adopts a Map/Set host, and a
    // non-collection host leaves the literal unmodeled.
    for body in [
        "List<int> f() => [];",
        "Map<String, int> f() => {};",
        "Set<int> f() => <int>{};",
        "Iterable<int> f() => [];",
        "dynamic f() => [];",
    ] {
        let report = unit_report(&format!("{harness}{body}"));
        assert_eq!(
            count_code(&report, "return_of_invalid_type"),
            0,
            "{body}: {:?}",
            code_names(&report)
        );
    }
}

/// The collection-vs-primitive negative is the matrix's shared
/// decision: an annotated set literal row (`Set<int>`) against an `int`
/// declaration rejects with the ordinary assignability face at the
/// literal (the same `return_of_invalid_type` decision one face up).
#[test]
fn annotated_set_literal_against_primitive_rejects() {
    let report =
        unit_report("class int {} class Set<T> {}\nvoid f() {\n  int x;\n  x = <int>{};\n  x;\n}");
    assert_eq!(
        count_code(&report, "invalid_assignment"),
        1,
        "{:?}",
        code_names(&report)
    );
}
/// #2144 probe: the `&&`-chain field promotion — a private final field
/// tested in a later conjunct promotes into the guarded block, through
/// an implicit-`this` read inside a getter (promotion_chains.dart's
/// `hasAnnotation`).
#[test]
fn and_chain_private_final_field_promotes_into_block() {
    let report = unit_report(
        "class String { bool get isNotEmpty => true; }\nclass C {\n  final String? _a;\n  final String? _r;\n  bool get has {\n    if (_a != null && _r != null) {\n      return _r.isNotEmpty;\n    }\n    return false;\n  }\n}",
    );
    assert_eq!(
        count_code(&report, "unchecked_use_of_nullable_value"),
        0,
        "{:?}",
        code_names(&report)
    );
}
/// #2144: a method tear-off's static type is the method's signature —
/// `var t = C<int>().m; t<A,B>` reports `wrong_number_of_type_arguments_
/// function` when the written count differs (broken_type_arguments.dart's
/// `handle<int, String>` on `M Function<M>(M)`). Without `tearoff_ty`
/// the local bound `Unknown` and the `_function` face slept.
#[test]
fn method_tearoff_instantiation_reports_function_arity() {
    let report = unit_report(
        "class int {} class String {}\nclass Box<T> {\n  M take<M>(M value) => value;\n}\nvoid open() {\n  final handle = Box<int>().take;\n  var bound = handle<int, String>;\n  print(bound);\n}",
    );
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments_function"),
        1,
        "{:?}",
        code_names(&report)
    );
    // The matching count stays silent.
    let ok = unit_report(
        "class int {} class String {}\nclass Box<T> {\n  M take<M>(M value) => value;\n}\nvoid open() {\n  final handle = Box<int>().take;\n  var bound = handle<int>;\n  print(bound);\n}",
    );
    assert_eq!(
        count_code(&ok, "wrong_number_of_type_arguments_function"),
        0,
        "{:?}",
        code_names(&ok)
    );
}

/// #2152: an alias chain through a named-type RHS resolves to its
/// terminal function shape — `typedef B = A<int, String>` over
/// `A<S, T> = T Function(S)` makes a `B`-typed local invocable
/// (`IntToStringN f; f(3)` in the torture `g006/g018/g030` files
/// reported `invocation_of_non_function_expression` while the alias
/// capture left `B`'s `Named` RHS unexpanded).
#[test]
fn named_alias_chain_to_fn_rhs_is_invocable() {
    let report = unit_report(
        "typedef Mapper6<S, T> = T Function(S);\n\
             typedef IntToString6 = Mapper6<int, String>;\n\
             String conv6(int n) => 'n$n';\n\
             String use6() {\n  final IntToString6 f = conv6;\n  return '${f(3)}';\n}",
    );
    assert_eq!(
        count_code(&report, "invocation_of_non_function_expression"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// #2152: a bare `Named`-typed callee whose class declares a `call`
/// method is invocable — `Adder` carries `int call(int, int)`, so
/// `sum(1, 2)` over `const Adder(10)` stays silent (the torture
/// `m011/m023/m035` files reported `invocation_of_non_function_expression`
/// without the member resolve).
#[test]
fn call_member_typed_callee_is_invocable() {
    let report = unit_report(
        "class int {}\n\
             class Adder {\n  const Adder([this.base = 0]);\n  final int base;\n  int call(int a, int b) => base + a + b;\n}\n\
             int use() {\n  const sum = Adder(10);\n  return sum(1, 2);\n}",
    );
    assert_eq!(
        count_code(&report, "invocation_of_non_function_expression"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// #2152: a parameterized function-type alias used as a local's
/// annotation is callable at the declared arity —
/// `Modern3<int> dbl; dbl(3)` stays silent and `Modern3<int>` itself
/// reports no `wrong_number_of_type_arguments` (the torture `m003`
/// family — a stale header latch once hid the typedef's own `<T>` from
/// its declared arity).
#[test]
fn parameterized_fn_alias_is_invocable() {
    let report = unit_report(
        "typedef Modern3<T> = T Function(T);\n\
             int use() {\n  Modern3<int> dbl = (n) => n * 2;\n  return dbl(3);\n}",
    );
    assert_eq!(
        count_code(&report, "invocation_of_non_function_expression"),
        0,
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// #2152: an extension type's representation variable is initialized by
/// its primary constructor — a redirecting generative constructor
/// (`S.fwd() : this(0)`) leaves `final_not_initialized` silent (the
/// torture `e006..e038` files reported the field face while every
/// generative ctor redirected).
#[test]
fn extension_type_rep_var_redirecting_ctor_stays_silent() {
    let report = unit_report(
        "extension type Seconds6(int v) {\n  Seconds6.fromMinutes(int m) : this(m * 60);\n}",
    );
    assert_eq!(
        count_code(&report, "final_not_initialized"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// #2177: a `super.name(...)` invocation in an initializer list binds
/// its labels against the resolved super-constructor's named formals —
/// a label the super constructor never declared reports
/// `undefined_named_parameter` at the label.
#[test]
fn super_named_argument_binds_against_super_ctor_formals() {
    let report = unit_report(
        "class B { const B({int good = 0}); }\n\
             class C extends B { const C() : super(bad: 1); }",
    );
    assert!(
        code_names(&report).contains(&"undefined_named_parameter"),
        "{:?}",
        code_names(&report)
    );
}

/// #2177: a label the super constructor *does* declare stays silent —
/// the same binding, the defined side.
#[test]
fn super_named_argument_defined_label_stays_silent() {
    let report = unit_report(
        "class B { const B({int good = 0}); }\n\
             class C extends B { const C() : super(good: 1); }",
    );
    assert_eq!(
        count_code(&report, "undefined_named_parameter"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// #2182: a super-constructor invocation's argument that is not
/// *potentially* constant — an unresolved name expression — reports
/// `invalid_constant` at the node (the oracle's
/// `_reportNotPotentialConstantsArguments`, constant_verifier.dart
/// :830-834,863-867).
#[test]
fn super_invocation_unresolved_arg_reports_invalid_constant() {
    let report = unit_report(
        "class B { const B(int t); }\n\
             class C extends B { const C() : super(missing_name); }",
    );
    assert!(
        code_names(&report).contains(&"invalid_constant"),
        "{:?}",
        code_names(&report)
    );
}

/// #2182: a genuinely constant super argument stays silent — a const
/// field read through the same arglist folds to a constant and never
/// reaches the collector.
#[test]
fn super_invocation_const_arg_stays_silent() {
    let report = unit_report(
        "const int k = 1;\n\
             class B { const B(int t); }\n\
             class C extends B { const C() : super(k); }",
    );
    assert_eq!(
        count_code(&report, "invalid_constant"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// #2178: a chained receiver carries its member's declared type —
/// `a.b.c` where `b` is a field of type `D` resolves `c` on `D`, so a
/// member `D` actually declares is not a false `undefined_method`.
#[test]
fn chained_receiver_field_type_resolves_member() {
    let report = unit_report(
        "class D { void c() {} }\n\
             class A { D b = D(); }\n\
             void t(A a) { a.b.c(); }",
    );
    assert_eq!(
        count_code(&report, "undefined_method"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// #2177: a getter-only member under a plain write spelling does NOT
/// report `undefined_setter` — the oracle routes it to
/// `assignment_to_final_no_setter` (the assignment-to-* family, not
/// modeled at this level). `undefined_setter` fires only when neither
/// `name` nor `name=` declares anything.
#[test]
fn getter_only_write_target_stays_off_undefined_setter() {
    let report = unit_report(
        "class C { int get onListen => 0; }\n\
             void t(C c) { c.onListen = 1; }",
    );
    assert_eq!(
        count_code(&report, "undefined_setter"),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// #2177: a totally undeclared write target reports `undefined_setter`
/// — neither the `name` nor the `name=` spelling resolved.
#[test]
fn missing_write_target_reports_undefined_setter() {
    let report = unit_report("class C {}\nvoid t(C c) { c.onListen = 1; }");
    assert!(
        code_names(&report).contains(&"undefined_setter"),
        "{:?}",
        code_names(&report)
    );
}

/// #2177: a declared setter satisfies the same write spelling — the
/// `name=` member resolves, so no `undefined_setter`.
#[test]
fn declared_setter_write_target_stays_silent() {
    let report = unit_report(
        "class C { set onListen(int v) {} }\n\
             void t(C c) { c.onListen = 1; }",
    );
    assert_eq!(
        count_code(&report, "undefined_setter"),
        0,
        "{:?}",
        code_names(&report)
    );
}
