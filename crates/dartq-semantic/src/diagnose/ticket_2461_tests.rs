//! The #2461 pins: chained member access through implicit-call sends
//! (census family A4, xml/petitparser/drift o-only rows).
//!
//! A value invocation `v(args)` whose static type names a container
//! declaring a `call` method resolves through that method — the invoked
//! expression's `call` element types the invocation node with the call
//! method's declared return (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/function_expression_invocation_
//! resolver.dart, the static-type dispatch at the send). The completed
//! send is the chain's next receiver, so `body(ctx).effectiveFlag`
//! resolves the member on `Expr.call`'s return type, never on the callee
//! value's own class — the bare-spelling face must read the callee's
//! static type the same way the merge-time member face does
//! ([`Engine::receiver_static_ty`]), not the information-less `row.ty` a
//! push-time bare read leaves.
//!
//! Oracle provenance: the census keys (Dart 3.13.1 over the xml corpus,
//! /tmp/probe2443) — `body(ctx).effectiveBooleanValue` /
//! `key!(ctx).atomize()` are silent there (the members exist on
//! `XPathSequence`, the call method's return), while dartq named the
//! callee's own class in the row's type slot. The rig declares no
//! imports, so every pin spells custom types only (`void`/`null` are
//! keywords and stay legal).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2449_tests` carries the same shape).
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

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

/// The census's statement.dart shape: a bare field read whose declared
/// type names a `call`-bearing class, invoked, with the member read on
/// the call's return. The send types through `Expr.call` to `Seq`, so
/// the getter resolves and the oracle is silent (xml
/// `body(currentContext).effectiveBooleanValue`).
#[test]
fn implicit_call_send_member_read_resolves() {
    let source = "\
class Ctx {}

class Seq {
  Flag get effectiveFlag => const Flag();
}

class Flag {
  const Flag();
}

abstract class Expr {
  Seq call(Ctx context);
}

class Some {
  final Expr body;

  const Some(this.body);

  Flag f(Ctx ctx) => body(ctx).effectiveFlag;
}
";
    let report = report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}

/// The census's lookup.dart shape: a nullable field, a `!` rewrite, then
/// the implicit-call send and a method read on its result —
/// `key!(context).atomize()` types through to `Seq.atomize()` and stays
/// silent.
#[test]
fn null_asserted_implicit_call_send_method_read_resolves() {
    let source = "\
class Ctx {}

class AtomList {
  AtomList atomize() => this;
}

abstract class Expr {
  AtomList call(Ctx context);
}

class Lookup {
  final Expr? key;

  const Lookup(this.key);

  AtomList g(Ctx context) => key!(context).atomize();
}
";
    let report = report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}

/// The typed-through miss: the member genuinely absent from the call's
/// return type reports `undefined_method` — the oracle types the send as
/// `Seq` and names that type in the row, so blanket suppression on the
/// face is as wrong as the old receiver-typed resolution.
#[test]
fn missing_member_on_implicit_call_result_reports() {
    let source = "\
class Ctx {}

class Seq {
  Flag get effectiveFlag => const Flag();
}

class Flag {
  const Flag();
}

abstract class Expr {
  Seq call(Ctx context);
}

class Some {
  final Expr body;

  const Some(this.body);

  void h(Ctx ctx) {
    body(ctx).missing();
  }
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "undefined_method"),
        1,
        "{:?}",
        messages(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'Seq'")),
        "the miss names the call's return type: {:?}",
        messages(&report)
    );
}

/// The two-hop chain: send, member read, null-assert, second implicit-call
/// send, member send (`body(ctx).key!(c).atomize()`) — every link types
/// through its predecessor's face, so the oracle is silent end to end.
#[test]
fn two_hop_implicit_call_chain_resolves() {
    let source = "\
class Ctx {}

class AtomList {
  AtomList atomize() => this;
}

abstract class Expr {
  Seq call(Ctx context);
}

class Seq {
  final Expr? key;
  final AtomList items;

  const Seq(this.key, this.items);

  AtomList atomize() => items;
}

class Some {
  final Expr body;

  const Some(this.body);

  AtomList f(Ctx ctx, Ctx c) => body(ctx).key!(c).atomize();
}
";
    let report = report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}

/// The generator shape from the census: `yield* body(ctx)` over a
/// `call`-bearing field — the send's `Seq` (a `Base`) is the yield-star
/// operand, and the live oracle reports the raw-face row (probed:
/// "The type 'Seq' implied by the 'yield*' expression must be assignable
/// to 'IterableLike'." — `impliedReturnType = expressionType`, the
/// send's resolved type, never a sequence wrap).
#[test]
fn yield_star_implicit_call_send_resolves() {
    let source = "\
class Ctx {}

class Base {
  const Base();
}

class Seq extends Base {
  const Seq();
}

abstract class Expr {
  Seq call(Ctx context);
}

class Some {
  final Expr body;

  const Some(this.body);

  IterableLike g(Ctx ctx) sync* {
    yield* body(ctx);
  }
}

class IterableLike {
  IterableLike() {
    append(const Seq());
  }

  void append(Base item) {}
}
";
    let report = report(source);
    let yields: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "yield_of_invalid_type")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        yields,
        ["The type 'Seq' implied by the 'yield*' expression must be assignable to 'IterableLike'."],
        "{:?}",
        messages(&report)
    );
}

/// The arity face over the implicit-call formals: `body()` misses `call`'s
/// required positional — the oracle resolves the send through `call` and
/// reports `not_enough_positional_arguments` there, so a silent send means
/// the face never armed.
#[test]
fn implicit_call_send_arity_face_arms() {
    let source = "\
class Ctx {}

class Seq {
  Flag get effectiveFlag => const Flag();
}

class Flag {
  const Flag();
}

abstract class Expr {
  Seq call(Ctx context);
}

class Some {
  final Expr body;

  const Some(this.body);

  void h(Ctx ignored) {
    body();
  }
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "not_enough_positional_arguments"),
        1,
        "{:?}",
        messages(&report)
    );
}

/// A local variable of a `call`-bearing class invokes through `call` the
/// same way a field does — the send's result is the call method's return,
/// so the chained read resolves exactly like the field-receiver shapes.
#[test]
fn local_implicit_call_send_types_through() {
    let source = "\
class Ctx {}

class Seq {
  Flag get ok => const Flag();
}

class Flag {
  const Flag();
}

class Expr {
  Seq call(Ctx context) => Seq();
}

void f(Ctx ctx) {
  Expr body = Expr();
  body(ctx).ok;
}
";
    let report = report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}

/// The function-face guard (the #2449 posture): a top-level *function*
/// send and a function-typed variable send stay on their own faces — the
/// static-type read must not arm the implicit-call face for a callable
/// whose formals already answer through the declaration or the written
/// signature, so both sends keep the prior silence.
#[test]
fn function_sends_keep_their_own_faces() {
    let source = "\
class Ctx {}

void top(Ctx ctx) {}

void f(Ctx ctx) {
  void Function(Ctx) cb = top;
  top(ctx);
  cb(ctx);
}
";
    let report = report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}
