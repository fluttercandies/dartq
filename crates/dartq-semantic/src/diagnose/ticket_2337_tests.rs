//! The #2337 pins: the `super.name` formal's associated-type face — a
//! positional forward reads the super constructor's parameter at its own
//! index among the super formals (`SuperFormalParameterElement.type`,
//! `element.dart` `indexIn`), a field-formal associated parameter takes
//! its field's declared type (the intl corpus's `super.locale` →
//! `this._locale` → `final String _locale` chain), a chained forward
//! climbs hop by hop, and a same-named local field never wins — so a
//! nullable argument at a creation site reports
//! `argument_type_not_assignable` against the associated non-nullable
//! parameter while the written annotation, the matching nullable
//! parameter, and the unassociated forward keep the silence. The
//! error-recovery operator companions: an invalid-chain receiver keeps
//! the binary node's fold unresolved (the oracle's
//! `undefinedChain - 0` silence — the unresolved-receiver arm), and a
//! known-typed `<<` operand reports the argument face.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole unit of a no-import library — the same
/// minimal rig `ticket_2324_tests` carries.
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

fn has_code(report: &FileReport, code: &str) -> bool {
    report.diagnostics.iter().any(|d| d.code.name == code)
}

// —— #2337 face ①: the super-formal associated type ——————————————————————

/// The probe's positive arm (`B(maybe, 'x')` over `B(super.s, super.q)`): a
/// positional `super.s` forward reads the super constructor's `this.s`
/// field-formal, whose type is the field's own `Txt` — the nullable
/// argument reports at the creation site.
#[test]
fn positional_super_formal_types_the_argument_face() {
    let source = "class Txt { Txt(); } \
                  class A { Txt s = Txt(); A(this.s); } \
                  class B extends A { B(super.s); } \
                  void f(Txt? maybe) { B(maybe); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}

/// The matching-parameter arm: a `Txt?` field-formal keeps the nullable
/// argument's silence — the association reads the parameter's declared
/// type, never a blanket non-null narrowing.
#[test]
fn nullable_associated_parameter_keeps_the_matching_argument() {
    let source = "class Txt { Txt(); } \
                  class A { Txt? q; A(this.q); } \
                  class B extends A { B(super.q); } \
                  void f(Txt? maybe) { B(maybe); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}

/// The written-annotation arm: `Txt? super.s` declares the parameter's own
/// type (upstream stores the written annotation over the association), so
/// the nullable argument fits even though the field is non-nullable.
#[test]
fn written_annotation_wins_over_the_association() {
    let source = "class Txt { Txt(); } \
                  class A { Txt s = Txt(); A(this.s); } \
                  class B extends A { B(Txt? super.s); } \
                  void f(Txt? maybe) { B(maybe); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}

/// The named-association arm: a `super.s` named forward associates by
/// spelling, so the nullable value under the `s:` label reports.
#[test]
fn named_super_formal_types_the_named_argument_face() {
    let source = "class Txt { Txt(); } \
                  class A { Txt s = Txt(); A({required this.s}); } \
                  class B extends A { B({required super.s}); } \
                  void f(Txt? maybe) { B(s: maybe); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}

/// The intl corpus's `super.locale` shape: the forward's name need not
/// match the associated parameter's (`this._locale`) — a positional
/// association reads the target's table by index, and the field-formal
/// parameter answers with its field's declared type.
#[test]
fn field_formal_association_answers_the_field_type() {
    let source = "class Txt { Txt(); } \
                  class N { Txt _locale = Txt(); N(this._locale); } \
                  class C extends N { C(super.locale); } \
                  void f(Txt? maybe) { C(maybe); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}

/// A same-named local field never wins: `D(super.s)` over a local
/// `Txt? s` field still reads the super constructor's `Txt` parameter —
/// the local field's nullability must not suppress the face.
#[test]
fn same_named_local_field_loses_to_the_association() {
    let source = "class Txt { Txt(); } \
                  class A { Txt s = Txt(); A(this.s); } \
                  class D extends A { Txt? s = null; D(super.s); } \
                  void f(Txt? maybe) { D(maybe); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}

/// The chained forward: `K(super.p)` → `M(super.p)` → `Z(this.p)` climbs
/// two associations, and the root field's `Txt` types the argument face.
#[test]
fn chained_super_formals_climb_to_the_root_field_type() {
    let source = "class Txt { Txt(); } \
                  class Z { Txt p = Txt(); Z(this.p); } \
                  class M extends Z { M(super.p); } \
                  class K extends M { K(super.p); } \
                  void f(Txt? maybe) { K(maybe); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}

/// The unassociated forward: `super.nope` over a single-parameter super
/// constructor associates to nothing — its parameter reads `InvalidType`
/// upstream, every question it touches suppresses, and the declaration's
/// own without-associated face is the only row.
#[test]
fn unassociated_super_formal_suppresses_the_argument_face() {
    let source = "class Txt { Txt(); } \
                  class A { Txt s = Txt(); A(this.s); } \
                  class E extends A { E(super.nope); } \
                  void f(Txt? maybe) { E(maybe); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
    assert!(
        has_code(&rep, "super_formal_parameter_without_associated_positional"),
        "{:?}",
        rep.diagnostics
    );
}

// —— #2337 face ②: the operator error-recovery arms ——————————————————————

/// The invalid-chain receiver arm (`viaUndef - 0` over an
/// undefined-getter chain): the receiver's type is unresolved, the
/// operator never resolves, and the binary node stays silent — the
/// oracle's own recovery posture (the `is int` test never promotes an
/// invalid-typed local).
#[test]
fn invalid_chain_receiver_keeps_the_binary_node_silent() {
    let source = "class Txt { Txt(); } \
                  class Sink { Txt call(Txt x) => x; } \
                  class U {} \
                  class C { \
                    Object? m(U node, Sink sink) { \
                      var viaUndef = node.rightOperand; \
                      if (viaUndef is int) { \
                        sink.call(viaUndef - 0); \
                      } \
                      return null; \
                    } \
                  }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}

/// The known-operand shift arm: a resolved `<<` argument whose static type
/// misses the operator's `int` formal reports at the operand — the
/// argument face the intl corpus's `1 << (64 - rightOperand)` rows ride
/// once the operand's own fold carries its static type.
#[test]
fn known_typed_shift_operand_reports_the_argument_face() {
    let source = "class Txt { Txt(); } \
                  Txt g() => Txt(); \
                  void f() { \
                    int a = 1 << g(); \
                    int b = 1 << 2.5; \
                  }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
    );
}
