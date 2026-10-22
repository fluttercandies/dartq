//! The #2324 pins: the dead-coalesce operand faces (`_checkForDeadNullCoalesce`'s
//! left-operand and `??=`-target verdicts) — a `?.`-merged read or send keeps
//! the null union at the right operand (`region?.offset ?? 0` stays silent,
//! the archive corpus's xz_decoder/zip_file rows), an invoked operand reads
//! its send's declared result (`c.m() ?? y` reports when the return is
//! non-nullable), and a generic key send never anchors the dead face. The
//! #2282 companion: an extension body's bare read of an `on`-type field
//! spares `unused_field` on the extended container (the revived
//! `extension_this_read_spares_the_on_type_field` pin carries the named
//! shape; the unnamed-extension spelling rides the same face). The #2338
//! companion: a member selector whose spelling shadows a promoted local
//! never adopts that local's promotion — the `!` verdict on the selector is
//! the member's own declared nullability. The #2339 companion: the shared
//! `unchecked_use_of_nullable_value` object face — an implicitly typed
//! closure formal under an unresolvable context schema binds `Object?` and
//! reports the unchecked code at member use, while a decidable member miss
//! through a merely-nullable receiver emits the same code where the
//! `undefined_*` trio used to fire. The #2346① companion: the non-closure
//! receiver faces — a for-in loop variable, a pattern destructure, a
//! nullable-scrutinee pattern binding — resolve their member verdicts
//! through the flow read-type face (the inferred-slot upgrade) and report
//! the shared unchecked code where the `undefined_*` family or silence
//! used to stand.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole unit of a no-import library — the same
/// minimal rig `ticket_2316_tests` carries.
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// `region?.offset ?? 0` (the archive corpus's xz_decoder.dart shape): the
/// `?.`-merged read keeps the null union, so the right operand is reachable
/// and no dead row reports even though the field itself is non-nullable.
#[test]
fn null_aware_read_lhs_keeps_the_dead_rhs_alive() {
    let source = "class int {} \
                  class Region { int offset = 0; } \
                  int f(Region? region) => region?.offset ?? 0;";
    let rep = report(source);
    assert!(
        !has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// `c?.m() ?? y`: a `?.`-merged send's result is nullable regardless of the
/// method's declared return — the right operand stays reachable.
#[test]
fn null_aware_invoke_lhs_keeps_the_dead_rhs_alive() {
    let source = "class int {} \
                  class C { int m() => 0; } \
                  int f(C? c, int y) => c?.m() ?? y;";
    let rep = report(source);
    assert!(
        !has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// `c.m() ?? y` over a non-nullable declared return: the send's static type
/// is the return type, the left operand can't be null, and the right
/// operand reports the dead face (the oracle's `_checkForDeadNullCoalesce`
/// invoke arm).
#[test]
fn invoked_non_null_lhs_reports_the_dead_rhs() {
    let source = "class int {} \
                  class C { int m() => 0; } \
                  int f(C c, int y) => c.m() ?? y;";
    let rep = report(source);
    assert!(
        has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// `c.m() ?? y` over a nullable declared return: the send's result can be
/// null — the right operand stays reachable.
#[test]
fn invoked_nullable_lhs_stays_silent() {
    let source = "class int {} \
                  class C { int? m() => 0; } \
                  int f(C c, int y) => c.m() ?? y;";
    let rep = report(source);
    assert!(
        !has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// `m[key(v)] ?? d`: the invoked key's nullable return never anchors the
/// dead face — the index read's own type (`Map.[]`'s `V?`) governs, and the
/// key send row is not the coalesce operand (#2172's `??`-face posture).
#[test]
fn invoked_key_never_anchors_the_dead_coalesce_rhs() {
    let source = "class Object {} class V {} \
                  class M { V? operator [](Object? k) => null; } \
                  Object? key(Object? o) => o; \
                  void f(M m, V v, V d) { var x = m[key(v)] ?? d; }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// An extension body's implicit-`this` read of an `on`-type field spares
/// the field: the unnamed-extension spelling rides the same owner face as
/// the named one (#2282's revived pin covers `extension X on C`).
#[test]
fn unnamed_extension_this_read_spares_the_on_type_field() {
    let source = "class C { \
                    int _n = 0; \
                  } \
                  extension on C { \
                    int get doubled => _n * 2; \
                  } \
                  void f(C c) { print(c.doubled); }";
    let rep = report(source);
    assert!(!has_code(&rep, "unused_field"), "{:?}", rep.diagnostics);
}

/// An extension body's bare read never spares the extension's OWN
/// same-spelled field through the `on` face: the owner is whichever
/// container the resolution answered, and a field only the extension
/// declares is the extension's own (the `on`-type climb must not leak the
/// credit to an unrelated container's field).
#[test]
fn extension_own_field_read_still_marks_the_extension() {
    let source = "class C { \
                    int _n = 0; \
                  } \
                  class D { \
                    int _n = 1; \
                  } \
                  extension X on C { \
                    int get doubled => _n * 2; \
                  } \
                  void f(C c, D d) { print(c.doubled); print(d._n); }";
    let rep = report(source);
    // C._n is read through the extension (spared); D._n is read directly
    // (spared). The pin: neither spelling leaks a false negative —
    // specifically C._n's credit must land on C, not D.
    assert!(!has_code(&rep, "unused_field"), "{:?}", rep.diagnostics);
}

// —— #2338: a member selector never adopts a shadowed local's promotion ——

/// `dep.dependencies!` where the selector's spelling shadows a promoted
/// local (the riverpod foundation.dart:162 shape): the selector is a member
/// read on the receiver chain — its `!` verdict is the field's declared
/// nullability (`POF?`), so the assertion is necessary and nothing reports.
/// The local's promotion (`POF`, non-null, after the `== null` guard) is
/// the local's own face; the name coincidence must not stage it at the
/// selector's span (probed: `dart analyze` stays silent, pre-fix dartq
/// fabricated `unnecessary_non_null_assertion` at the `!`).
#[test]
fn member_selector_shadow_of_promoted_local_stays_silent() {
    let source = "class POF { POF? dependencies; } \
                  void f(POF dep) { \
                    POF? dependencies = null; \
                    if (dependencies == null) return; \
                    dep.dependencies!; \
                  }";
    let rep = report(source);
    assert!(rep.diagnostics.is_empty(), "{:?}", rep.diagnostics);
}

/// The closure-captured variant: the shadowed local is captured by a
/// closure after its promotion (the riverpod consumer shape). The capture
/// changes nothing — the selector still reads the field, and the
/// fabrication must stay dead. The closure variable itself stays unused in
/// both engines (the only row the oracle reports here).
#[test]
fn captured_local_shadow_variant_stays_silent() {
    let source = "class POF { POF? dependencies; } \
                  void f(POF dep) { \
                    POF? dependencies = null; \
                    if (dependencies == null) return; \
                    var g = () => dependencies; \
                    dep.dependencies!; \
                  }";
    let rep = report(source);
    assert_eq!(
        code_names(&rep),
        ["unused_local_variable"],
        "{:?}",
        code_names(&rep)
    );
}

/// The staged `!` face itself stays alive: a bare local's own read —
/// declared nullable, promoted non-null by the guard — stages at its own
/// span and the teardown reports `unnecessary_non_null_assertion`. This is
/// the arm the shadow gate preserves (the receiver-chain gate never
/// touches a bare operand).
#[test]
fn local_assert_after_promotion_still_reports() {
    let source = "class int {} \
                  void f() { \
                    int? x = null; \
                    if (x == null) return; \
                    x!; \
                  }";
    let rep = report(source);
    assert_eq!(
        code_names(&rep),
        ["unnecessary_non_null_assertion"],
        "{:?}",
        code_names(&rep)
    );
}

/// Boundary, no shadow: the member read's own nullability governs as
/// usual — the necessary `!` on a `POF?` field stays silent, and the same
/// read without the assertion reports the nullable dereference.
#[test]
fn member_nullability_without_shadow_unchanged() {
    let asserted = "class POF { POF? dependencies; POF? first; } \
                    void f(POF dep) { dep.dependencies!; }";
    let rep = report(asserted);
    assert!(rep.diagnostics.is_empty(), "{:?}", rep.diagnostics);
    let bare = "class POF { POF? dependencies; POF? first; } \
                void f(POF dep) { dep.dependencies.first; }";
    let rep = report(bare);
    assert_eq!(
        code_names(&rep),
        ["unchecked_use_of_nullable_value"],
        "{:?}",
        code_names(&rep)
    );
}

// —— #2339: the `unchecked_use_of_nullable_value` object face, both halves
// of the predicate ————————————————————————————————————————————————————
//
// The inference half (`function_expression_resolver.dart`
// `_inferFormalParameters`): an implicitly typed formal of an open
// function literal infers its schema entry under `greatestClosure`, and a
// schema entry that is `T <: Null` — the oracle's `InvalidType`, an
// unresolvable head or type argument — binds `Object?`. A member use
// inside the literal then reports the shared unchecked code at the
// selector. Every receiver the oracle's `isDynamicBounded`/
// `isInvalidBounded` arms bound first stays silent.
//
// The emission half (`type_property_resolver.dart`'s `isNullable` tail):
// a decidable member miss through a merely-nullable receiver reports the
// shared `unchecked_use_of_nullable_value` code at the selector — the
// oracle's three variant templates all carry that one code name — and the
// invoke intent joins the found-arm's nullable-deref gate.

/// `take((s) => s.foo())` where `take`'s formal is `void Function(Und)` —
/// the written signature table answers the schema (rule 0c poisons the
/// element model's function-typed slot), `Und` fails `named_type_is_type`,
/// and `T <: Null` collapses the formal to `Object?`: the member use
/// inside the literal reports the shared unchecked code.
#[test]
fn closure_formal_under_invalid_schema_binds_object_question() {
    let source = "void take(void Function(Und) f) {} \
                  void main() { take((s) => s.foo()); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// The property-access face of the same collapse: `(s) => s.length`
/// reports the same shared code at the selector (the oracle's property
/// variant — one code name for all three shapes).
#[test]
fn closure_formal_property_access_reports_the_same_code() {
    let source = "void take(void Function(Und) f) {} \
                  void main() { take((s) => s.length); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// The aliased written signature answers through the same chain —
/// `typedef F = void Function(Und)` unfolds at `closure_schema_sig`'s
/// `alias_fn_sig` arm and still binds `Object?`.
#[test]
fn aliased_function_schema_still_collapses() {
    let source = "typedef F = void Function(Und); \
                  void take(F f) {} \
                  void main() { take((s) => s.foo()); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// A resolvable schema never reaches `greatestClosure`'s collapse — under
/// `List<String>` the literal's `e` keeps `dynamic` (no inference), and
/// `e.length` stays silent exactly like the oracle's `String`-bound read.
#[test]
fn valid_context_schema_never_infers() {
    let source = "class String { int get length => 0; } \
                  class List<T> {} \
                  void f(List<String> xs) { xs.map((e) => e.length); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// A schema head that is valid but locally unnameable — an imported class,
/// the declaring container's own parameter — under a NON-GENERIC receiver
/// (no written type argument to substitute) never reaches
/// `greatestClosure`'s `Object?` collapse either: that collapse is for
/// types invalid AS WRITTEN (`T <: Null`, an unresolvable annotation), and
/// nothing invalid was written. The formal keeps `dynamic` and the member
/// use stays silent, exactly where the oracle binds the real type (drift's
/// `db.batch((b) => b.insertAll(...))` — `Batch` through the import on the
/// non-generic connection holder — and `stmt.map((row) => row.read(...))`
/// — the container's `T` with no instantiation to read).
#[test]
fn non_generic_receiver_unnameable_schema_stays_dynamic() {
    let source = "class Conn { void batch(void Function(Batch) f) {} }\n\
                  void f(Conn db) {\n\
                    db.batch((b) { b.insertAll(); });\n\
                  }\n";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// The container's own parameter is the receiver's to instantiate — a
/// receiver face carrying no written type arguments leaves the head
/// unnameable, and the formal keeps `dynamic` (silent): never the
/// fresh-parameter `Object?` face, which belongs to the callee's own
/// parameters alone (drift's `Selectable<N> map<N>(N Function(T)
/// mapper)` under the arg-less statement face).
#[test]
fn container_param_schema_under_argless_receiver_stays_dynamic() {
    let source = "class Sel<T> { void map<N>(N Function(T row) mapper) {} }\n\
                  void f(Sel sel) {\n\
                  sel.map((row) { row.read(); });\n\
                  }\n";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// A callee-own parameter under a nullable-writable formal solves from
/// the typed argument with the formal's own `?` absorbing the argument's
/// nullability: official `A? <: T?` binds `T` the argument's NON-NULL
/// head (probed 3.13.1: a resolved `Tok2?` argument types the callback
/// formal `Tok2` — `n.lexeme` is `UNDEFINED_GETTER on 'Tok2'`, never an
/// unchecked row). The solved head is the oracle's silent arm — the
/// engine binds `dynamic` (its member walk under-resolves the
/// extension/import members the oracle resolves through, the
/// manifest_item `items.write` family).
#[test]
fn nullable_formal_absorbs_arg_nullability() {
    let source = "class BW { void take<T>(T? o, void Function(T x) w) {} }\n\
                  class V { int get flag => 1; }\n\
                  void f(BW bw, V? v) {\n\
                    bw.take(v, (x) { x.flag; });\n\
                  }\n";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}


/// A bare `this` argument carries the enclosing extension's on-type —
/// `extension on M2?` contributes `M2?`, whose nullability the formal's
/// `?` absorbs, so the callback formal stays silent on member uses
/// (manifest_id.dart:150/157's `writeOptionalObject(this, (it) =>
/// it.write(writer))`).
#[test]
fn extension_this_arg_solves_on_type() {
    let source = "class BW { void take<T>(T? o, void Function(T x) w) {} }\n\
                  class M2 { void write() {} }\n\
                  extension E on M2? {\n\
                    void go(BW bw) {\n\
                      bw.take(this, (x) { x.write(); });\n\
                    }\n\
                  }\n";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// An annotated invalid formal is `InvalidType` itself — the
/// `isInvalidBounded` arm answers before `isNullable` runs, so `x.foo()`
/// stays silent. (`Und` still reports its own `undefined_class` face.)
#[test]
fn annotated_invalid_formal_stays_silent() {
    let source = "void f(Und x) { x.foo(); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "undefined_method"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "undefined_getter"),
        "{:?}",
        code_names(&rep)
    );
}

/// A for-in variable over an invalid container binds `InvalidType` — the
/// same bounded silence holds.
#[test]
fn for_in_over_invalid_container_stays_silent() {
    let source = "class List<T> {} \
                  void f(List<Und> xs) { for (final x in xs) { x.foo(); } }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// A local initialized from an unresolved name binds `Unknown` — the
/// suppress-everything face, never the nullable face.
#[test]
fn invalid_initializer_local_stays_silent() {
    let source = "void f() { var x = Und; x.foo(); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// `List<Und>.first`'s declared type is `InvalidType` — the `.foo()` send
/// on it stays silent.
#[test]
fn invalid_first_chain_stays_silent() {
    let source = "class List<T> { T get first => throw 0; } \
                  void f(List<Und> xs) { xs.first.foo(); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// The `isNullable` tail: a member miss on `Object?` reports the shared
/// unchecked code at the selector — never `undefined_method` (upstream's
/// miss path emits the unchecked variant, not the undefined family).
#[test]
fn nullable_receiver_miss_reports_unchecked_not_undefined() {
    let source = "void f(Object? y) { y.runUpdate(1); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "undefined_method"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "undefined_getter"),
        "{:?}",
        code_names(&rep)
    );
}

/// The property face of the same tail (`y.length` on `Object?`) — the
/// miss reports the shared code, not `undefined_getter`.
#[test]
fn nullable_receiver_property_miss_reports_unchecked() {
    let source = "void f(Object? y) { y.length; }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "undefined_getter"),
        "{:?}",
        code_names(&rep)
    );
}

/// A member miss on a declared nullable class (`C? c; c.undef()`) — the
/// `isNullable` tail again: unchecked, never `undefined_method`.
#[test]
fn nullable_class_miss_reports_unchecked() {
    let source = "class C {} \
                  void f(C? c) { c.undef(); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "undefined_method"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "undefined_getter"),
        "{:?}",
        code_names(&rep)
    );
}

/// The invoke intent joins the found-arm gate: `C? c; c.m()` where `m`
/// resolves on `C` still reads a possibly-null receiver — the oracle's
/// unchecked row, which the old `!Intent::Invoke` carve-out dropped.
#[test]
fn nullable_receiver_invoke_reports_unchecked() {
    let source = "class C { void m() {} } \
                  void f(C? c) { c.m(); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// The non-nullable boundary: `Object y; y.runUpdate(1)` reports the
/// `undefined_*` family — the unchecked tail needs a nullable receiver.
#[test]
fn non_nullable_miss_keeps_the_undefined_family() {
    let source = "void f(Object y) { y.runUpdate(1); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(has_code(&rep, "undefined_method"), "{:?}", code_names(&rep));
}

// —— #2346① companion: the non-closure UNCHECKED receiver faces — a
// receiver whose declared slot the name-level model never bound (the
// for-in loop variable, a pattern destructure) reads its static type from
// the flow layer's read-type row; a nullable face reports the shared
// unchecked-access code at the member verdict, never the `undefined_*`
// family.

/// The for-loop element receiver (`for (var e in l) e.m()` over
/// `List<C?>`): the loop variable's inferred slot resolves through the
/// flow read-type face — the nullable element type reports the unchecked
/// code at the send, never `undefined_method` (upstream's
/// `_inferForEachVariable` element type).
#[test]
fn for_in_element_nullable_receiver_reports_unchecked() {
    let source = "class C { void m() {} } \
                  class List<T> {} \
                  void f(List<C?> l) { for (var e in l) { e.m(); } }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_method"), "{:?}", code_names(&rep));
}

/// The read face of the same inferred-slot receiver (`e.length` on the
/// nullable element): the miss reports the unchecked code, never
/// `undefined_getter`.
#[test]
fn for_in_element_read_reports_unchecked() {
    let source = "class C { void m() {} } \
                  class List<T> {} \
                  void f(List<C?> l) { for (var e in l) { e.length; } }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_getter"), "{:?}", code_names(&rep));
}

/// The write face (`e.x = 1` through the nullable element): the unchecked
/// code again — the `isNullable` tail precedes the write trio.
#[test]
fn for_in_element_write_reports_unchecked() {
    let source = "class C { void m() {} } \
                  class List<T> {} \
                  void f(List<C?> l) { for (var e in l) { e.x = 1; } }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_setter"), "{:?}", code_names(&rep));
}

/// The non-nullable boundary of the inferred-slot face: an element of
/// `List<C>` resolves the member on `C` — no unchecked row, no
/// `undefined_*` family.
#[test]
fn for_in_element_non_null_stays_silent() {
    let source = "class C { void m() {} } \
                  class List<T> {} \
                  void f(List<C> l) { for (var e in l) { e.m(); } }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "undefined_method") && !has_code(&rep, "undefined_getter"),
        "{:?}",
        code_names(&rep)
    );
}

/// The record-destructure receiver (`var (c, n) = r` on a `(C?, int)`
/// parameter): the flat-record bind's positional slot type is nullable —
/// the member verdict reports the unchecked code.
#[test]
fn record_destructure_nullable_slot_reports_unchecked() {
    let source = "class C { void m() {} } \
                  void f((C?, int) r) { var (c, n) = r; c.m(); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_method"), "{:?}", code_names(&rep));
}

/// The non-nullable record slot: `(C, int)`'s `c` is `C` — the member
/// resolves and nothing reports.
#[test]
fn record_destructure_non_null_stays_silent() {
    let source = "class C { void m() {} } \
                  void f((C, int) r) { var (c, n) = r; c.m(); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_method"), "{:?}", code_names(&rep));
}

/// The if-case pattern variable under a nullable scrutinee (`Object? o`
/// case `C? p`): the null branch is reachable, the bound variable keeps
/// the written nullable type, and the member access reports the unchecked
/// code.
#[test]
fn if_case_pattern_nullable_scrutinee_reports_unchecked() {
    let source = "class C { void m() {} } \
                  void f(Object? o) { if (o case C? p) { p.m(); } }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_method"), "{:?}", code_names(&rep));
}

/// The switch-case pattern binding under a nullable scrutinee: the bound
/// variable's written `C?` rides the same predicate path.
#[test]
fn switch_case_pattern_nullable_scrutinee_reports_unchecked() {
    let source = "class C { void m() {} } \
                  void f(Object? o) { switch (o) { case C? c: c.m(); } }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_method"), "{:?}", code_names(&rep));
}

/// The dynamic-dispatch boundary of the inferred-slot face: an untyped
/// formal's flow face is `dynamic` — never a named container — so the
/// member access keeps the silent dispatch (`unk.m()` reports nothing).
#[test]
fn inferred_slot_dynamic_face_keeps_the_silent_dispatch() {
    let source = "class C { void m() {} } \
                  void f(unk) { unk.m(); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_method"), "{:?}", code_names(&rep));
}

// —— #2347: the closure-formal schema's receiver-element substitution ———————
//
// `function_expression_resolver.dart`'s `_inferFormalParameters`: an
// implicitly typed formal of an open function literal infers its schema
// entry under `greatestClosure`. A schema entry that names a type the file
// cannot prove — a declaration's own type parameter (`forEach`'s `T`, the
// callee's `E`) — is not the oracle's `InvalidType` collapse: the oracle
// substitutes the receiver's instantiated element before answering, so
// `Bag<Node>.forEach((e) => e.copy())` binds `e` to `Node`, not `Object?`.
// The receiver's first written argument stands in for that substitution at
// name level: a decidable non-null named element or a `dynamic` one reads
// the oracle's silent arm, while a receiver argument the file cannot name
// keeps the `Object?` recovery (the `Und`-schema pins' verdict — the same
// invalid face the unresolved head already carried).

/// The receiver-first-argument substitution: `bag.forEach((e) =>
/// e.copy())` where `forEach`'s written schema names a type the file
/// cannot prove (`void Function(T)`'s `T`, a declaration's own type
/// parameter) — the oracle substitutes the receiver's instantiated
/// element before answering. A decidable non-null named element binds the
/// formal to the silent arm, never `Object?`.
#[test]
fn generic_receiver_closure_formal_substitutes_the_element() {
    let source = "class Node { Node copy() => Node(); } \
                  class Bag<T> { void forEach(void Function(T) f) {} } \
                  void f(Bag<Node> bag) { bag.forEach((e) => e.copy()); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// A `dynamic` element never infers: `List<dynamic>`'s closure formal
/// binds `dynamic` and the member use stays silent — the receiver-first-
/// argument substitution's `Dynamic` arm.
#[test]
fn dynamic_element_closure_formal_never_infers() {
    let source = "class Bag<T> { void forEach(void Function(T) f) {} } \
                  void f(Bag<dynamic> bag) { bag.forEach((e) => e.copy()); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// An invalid written element never reaches the closure formal:
/// `Bag<Und>`'s `Und` is not a type, so the receiver's type-argument list
/// collapses to `InvalidType` and the `void Function(T)` schema's `T`
/// substitutes the invalid face — `greatestClosure` binds `e` `Object?`,
/// and `e.copy()` is the oracle's invalid-bounded arm: silent on the
/// unchecked code. The `Und` argument itself reports
/// `non_type_as_type_argument`; the member use adds nothing.
#[test]
fn invalid_element_argument_keeps_the_object_question_recovery() {
    let source = "class Bag<T> { void forEach(void Function(T) f) {} } \
                  void f(Bag<Und> bag) { bag.forEach((e) => e.copy()); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        has_code(&rep, "non_type_as_type_argument"),
        "{:?}",
        code_names(&rep)
    );
}
