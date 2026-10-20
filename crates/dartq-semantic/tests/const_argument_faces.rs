//! Pins for the const-domain argument faces:
//!
//! * the `dart:async` `Stream.empty` const factory redirect answers the
//!   implicit-const question like its `dart:core` siblings in
//!   [`CORE_CONST_CTOR`] — `const Stream.empty()` never fails the
//!   enclosing declaration (upstream `sdk/lib/async/stream.dart:165`,
//!   `const factory Stream.empty() = _EmptyStream<T>`);
//! * a const creation nested inside another const creation anchors its
//!   `invalid_constant` at the inner unresolved argument, never at the
//!   creation head (`C(C.n(Missing()))` reports at `Missing()`);
//! * a const list literal reports `non_constant_list_element` once per
//!   unresolved element at that element's own span — an unresolved call
//!   element following a sibling keeps its own span instead of inheriting
//!   the literal's;
//! * a const generic creation argument is the creation's value, not its
//!   inner literal: `isExp(const Constant(null))` under `Expression<D>`
//!   checks `Constant`, so the `null` never leaks an
//!   `argument_type_not_assignable` row (the oracle's downward inference
//!   owns the creation's own argument faces);
//! * a member value invocation over a type declaring `call` resolves the
//!   implicit `.call`: the arguments check against the call method's
//!   formals and the invocation's static type is the call method's
//!   return (`body(inner)` over `XPathExpression body` yields
//!   `XPathSequence`).
//!
//! Each fixture mirrors a probed oracle verdict (fvm default `dart
//! analyze`, Dart 3.x).

use std::path::Path;

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::collector::collect;
use dartq_semantic::diagnose::{StrictModes, UnitFold, fold_unit};
use dartq_semantic::library::ElementIndex;
use dartq_semantic::{FileReport, MicroKind};
use dartq_source::{FileTable, StringTable};

/// The synthetic dart:core names these fragments consume (same
/// provenance posture as the sibling rigs' SDK faces). `Stream` stands in
/// for the unindexed SDK container its const factory faces key on.
const CORE_FACE: &[&str] = &["Object", "bool", "double", "int", "String", "Stream"];

/// Runs `source` through the live pipeline (the corpus posture: language
/// 3.13, a standalone library over the synthetic core face).
fn unit_fold(source: &str, strings: &StringTable) -> UnitFold {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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
    let elements = collect(source, &scanned.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
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
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    fold_unit(
        source,
        &scanned.tokens,
        features,
        strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &dartq_resolution::LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// The fragment's rows for `codes` as `(code, offset)` pairs in source
/// order — offsets, not lines: these faces diverge inside one line (the
/// nested-creation anchor and the per-element literal spans).
fn code_offsets(report: &FileReport, codes: &[&str]) -> Vec<(String, u32)> {
    let mut rows: Vec<(String, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| codes.contains(&d.code.name))
        .map(|d| (d.code.name.to_string(), d.range.offset))
        .collect();
    rows.sort();
    rows
}

/// The byte offset of `needle`'s first occurrence in `source`.
fn offset_of(source: &str, needle: &str) -> u32 {
    u32::try_from(
        source
            .find(needle)
            .unwrap_or_else(|| panic!("needle {needle:?} absent from fragment")),
    )
    .expect("offset fits u32")
}

const STREAM_EMPTY_CONST: &str = "\
void m() {
  const z = Stream.empty();
}
";

#[test]
fn stream_empty_const_factory_is_a_valid_constant() {
    let strings = StringTable::new();
    let fold = unit_fold(STREAM_EMPTY_CONST, &strings);
    let got = code_offsets(
        &fold.report,
        &[
            "invalid_constant",
            "const_initialized_with_non_constant_value",
            "const_with_non_const",
        ],
    );
    assert_eq!(
        got, vec![],
        "the upstream const factory redirect answers the implicit-const \
         question — the declaration stays silent"
    );
}

const NESTED_CREATION_SPAN: &str = "\
class C {
  const C(Object? a, [Object? b]);
  const C.n(Object? a);
}

void m() {
  const z = C(C.n(Missing()));
}
";

#[test]
fn nested_creation_failure_anchors_at_the_inner_argument() {
    let strings = StringTable::new();
    let fold = unit_fold(NESTED_CREATION_SPAN, &strings);
    let got = code_offsets(&fold.report, &["invalid_constant"]);
    assert_eq!(
        got,
        vec![(
            "invalid_constant".to_string(),
            offset_of(NESTED_CREATION_SPAN, "Missing()"),
        )],
        "the oracle's evaluation fails at the inner unresolved argument — \
         one row, anchored at `Missing()`, never at the creation head"
    );
}

const LIST_ELEMENT_SPANS: &str = "\
void m() {
  const z = [missing, Missing()];
}
";

#[test]
fn const_list_reports_each_unresolved_element_at_its_own_span() {
    let strings = StringTable::new();
    let fold = unit_fold(LIST_ELEMENT_SPANS, &strings);
    let got = code_offsets(&fold.report, &["non_constant_list_element"]);
    assert_eq!(
        got,
        vec![
            (
                "non_constant_list_element".to_string(),
                offset_of(LIST_ELEMENT_SPANS, "missing"),
            ),
            (
                "non_constant_list_element".to_string(),
                offset_of(LIST_ELEMENT_SPANS, "Missing()"),
            ),
        ],
        "one face per element, each at the element's own span — the call \
         element after a sibling keeps its own span, never the literal's"
    );
}

const CONST_CREATION_ARGUMENT: &str = "\
class Expression<T extends Object> {
  final T? value;
  const Expression(this.value);
}

class Constant<T extends Object> extends Expression<T> {
  final T? value;
  const Constant(this.value);
}

Expression<bool> isExp(Expression<bool> other) => other;

Expression<bool> r16() => isExp(const Expression(null));
";

#[test]
fn const_creation_argument_checks_the_creation_not_the_inner_literal() {
    let strings = StringTable::new();
    let fold = unit_fold(CONST_CREATION_ARGUMENT, &strings);
    let got = code_offsets(&fold.report, &["argument_type_not_assignable"]);
    assert_eq!(
        got, vec![],
        "the oracle's downward inference instantiates `Expression<bool>` \
         from the context, so the `null` fits `bool?` — no argument face \
         against the outer parameter leaks from the inner literal"
    );
}

const CONST_GENERIC_METHOD_CONTEXT: &str = "\
class Expression<T extends Object> {
  const Expression();
}

class Constant<T extends Object> extends Expression<T> {
  final T? value;
  const Constant(this.value);
}

class Column<D extends Object> {
  Expression<bool> isExp(Expression<D> other) => other;
  Expression<bool> isNull() => isExp(const Constant(null));
}
";

#[test]
fn const_creation_argument_silent_under_the_class_type_variable_context() {
    let strings = StringTable::new();
    let fold = unit_fold(CONST_GENERIC_METHOD_CONTEXT, &strings);
    let got = code_offsets(&fold.report, &["argument_type_not_assignable"]);
    assert_eq!(
        got, vec![],
        "the drift corpus shape: `Expression<D>`'s context leaves the \
         creation's own `null` argument to the creation's faces — the \
         outer call stays silent"
    );
}

const IMPLICIT_CALL_ARG_CHECK: &str = "\
class XPathSequence {}
class XPathContext {}

class XPathExpression {
  XPathSequence call(XPathContext context) => XPathSequence();
}

class Ctx {
  XPathExpression get expression => XPathExpression();
}

void r19(Ctx ctx) {
  ctx.expression(ctx);
}
";

#[test]
fn implicit_call_checks_arguments_against_the_call_formals() {
    let strings = StringTable::new();
    let fold = unit_fold(IMPLICIT_CALL_ARG_CHECK, &strings);
    let got = code_offsets(&fold.report, &["argument_type_not_assignable"]);
    assert_eq!(
        got,
        vec![(
            "argument_type_not_assignable".to_string(),
            offset_of(IMPLICIT_CALL_ARG_CHECK, "ctx);"),
        )],
        "the oracle resolves the implicit `.call` and checks the argument \
         against `XPathContext` — the deliberately wrong `Ctx` argument \
         reports at its own span"
    );
}

const IMPLICIT_CALL_RETURN_TYPE: &str = "\
class XPathSequence {}
class XPathContext {}

class XPathExpression {
  XPathSequence call(XPathContext context) => XPathSequence();
}

class Host {
  final XPathExpression body;
  Host(this.body);

  XPathSequence eval(XPathContext inner) {
    return body(inner);
  }
}
";

#[test]
fn implicit_call_yields_the_call_methods_return_type() {
    let strings = StringTable::new();
    let fold = unit_fold(IMPLICIT_CALL_RETURN_TYPE, &strings);
    let got = code_offsets(&fold.report, &["return_of_invalid_type"]);
    assert_eq!(
        got, vec![],
        "the invocation's static type is `XPathSequence` (the call \
         method's return), so the return check passes — the receiver's \
         own type must not leak into the return face"
    );
}
