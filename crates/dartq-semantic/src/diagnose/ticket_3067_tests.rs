//! The #3067 pins — suspends-body closure literals keep their capture,
//! and a no-context untyped formal infers `dynamic` instead of poisoning
//! the capture.
//!
//! Two gaps in `type_capture.rs` silenced every ATNA row a suspends
//! (async/generator) literal or a no-context untyped-formal literal
//! should produce:
//!
//! - `closure_capture_end` poisoned the whole capture under an
//!   async/generator frame. Upstream the literal is still a
//!   `FunctionExpression` with a static type — the body modifier only
//!   WRAPS the return: `async` through `Future<flatten(R)>`
//!   (`body_inference_context.dart:132-153` `computeInferredReturnType`,
//!   already mirrored by `note_async_modifier`'s capture-ctx rewrite), a
//!   generator through `Iterable<T>`/`Stream<T>` (the element type
//!   infers from the yields; the empty body answers `dynamic`). The
//!   `void` stub's re-type-to-formal-return mechanism is the sync cap
//!   (`S = R`) and is false for these literals, so they skip the stub
//!   and decide through the typed row's wrapped face (the argument ask
//!   reads each argument's resolved `staticType`,
//!   `resolver.dart:519-536`).
//! - `closure_formal` poisoned the capture when an untyped formal found
//!   no context. Upstream `inferLocalFunction` with no
//!   formal-inference source keeps the parameter `dynamic` — the
//!   literal's own signature still lands (`(x) {}` with no context
//!   answers `dynamic Function(dynamic)`, the A/B for the bound faces
//!   below).
//!
//! Shapes A/B'd against `dart analyze` (3.13 oracle): async-enclosed
//! plain literals report unchanged, async/generator literals report
//! through the wrapped face, `Iterable`/`Stream` destinations stay
//! silent, and untyped-formal literals report against non-function
//! formals exactly like the oracle.

use super::test_rig::report_ops;
use super::*;

fn codes(folded: &FileReport) -> Vec<&str> {
    folded
        .diagnostics
        .iter()
        .map(|diag| diag.code.name)
        .collect()
}

fn single(folded: &FileReport) -> &str {
    assert_eq!(
        folded.diagnostics.len(),
        1,
        "exactly one diagnostic: {:#?}",
        folded.diagnostics
    );
    folded.diagnostics[0].code.name
}

/// `g(() async {})` against `void Function(dynamic)` — the async literal's
/// static type wraps to `Future<void> Function()`, and the zero-formal
/// arity deficit reports (the oracle: `'Future<void> Function()' can't be
/// assigned to 'void Function(dynamic)'`).
#[test]
fn async_literal_arity_deficit_reports() {
    let folded = report_ops(
        "void g(void Function(dynamic) f) {}\n\
         void k() {\n\
           g(() async {});\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "async literal keeps its capture and reports the arity deficit: {:#?}",
        folded.diagnostics
    );
}

/// `g(() async {})` against `int Function()` — perfect arity, but the
/// wrapped return `Future<…>` never assigns into `int` (the oracle:
/// `'Future<Null> Function()'`). The stub's re-type would have silenced
/// exactly this row. The fixture declares its own `Future` head (the
/// #3061 pattern) so the wrapped face is a decidable rejection rather
/// than an undecidable unindexed-SDK head.
#[test]
fn async_literal_return_mismatch_reports() {
    let folded = report_ops(
        "class Future<T> {}\n\
         void g(int Function() f) {}\n\
         void k() {\n\
           g(() async {});\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "the wrapped return rejects against the int-return formal: {:#?}",
        folded.diagnostics
    );
}

/// `g(() async => 3)` against `int Function()` — the body's `int` return
/// wraps to `Future<int>` and still rejects (the oracle spells the same
/// `'Future<int> Function()'` face).
#[test]
fn async_literal_return_expression_reports() {
    let folded = report_ops(
        "class Future<T> {}\n\
         void g(int Function() f) {}\n\
         void k() {\n\
           g(() async => 3);\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "async wrap of the body's int return rejects: {:#?}",
        folded.diagnostics
    );
}

/// The void-destination twin stays silent: `g(() async {})` against
/// `void Function()` — `Future<…>` assigns into `void`.
#[test]
fn async_literal_void_context_stays_silent() {
    let folded = report_ops(
        "void g(void Function() f) {}\n\
         void k() {\n\
           g(() async {});\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`Future<…> Function()` assigns into `void Function()`: silent"
    );
}

/// A no-context async literal stays silent: the untyped formal supplies
/// no function-shaped context, so the capture produces no typed row (the
/// oracle's `h(() async {})` over `h(g)` is silent too).
#[test]
fn async_literal_no_context_stays_silent() {
    let folded = report_ops(
        "void h(g) {}\n\
         void k() {\n\
           h(() async {});\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "no context, no row — the same silence as the oracle"
    );
}

/// `g(() sync* {})` against `void Function(dynamic)` — the generator's
/// static return is `Iterable<dynamic> Function()` and the arity deficit
/// reports (the oracle spells `'Iterable<dynamic> Function()'`).
#[test]
fn sync_star_literal_arity_deficit_reports() {
    let folded = report_ops(
        "void g(void Function(dynamic) f) {}\n\
         void k() {\n\
           g(() sync* {});\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "sync* literal reports through the Iterable face: {:#?}",
        folded.diagnostics
    );
}

/// `g(() sync* {})` against `int Function()` — `Iterable<dynamic>` never
/// assigns into `int` (the oracle: `'Iterable<dynamic> Function()'`).
#[test]
fn sync_star_literal_return_mismatch_reports() {
    let folded = report_ops(
        "void g(int Function() f) {}\n\
         void k() {\n\
           g(() sync* {});\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "the generator wrap rejects against the int-return formal: {:#?}",
        folded.diagnostics
    );
}

/// The element-typed destination stays silent: `g(() sync* {})` against
/// `Iterable<int> Function()` — `Iterable<dynamic>` assigns (the oracle
/// is silent on the same shape).
#[test]
fn sync_star_iterable_context_stays_silent() {
    let folded = report_ops(
        "void g(Iterable<int> Function() f) {}\n\
         void k() {\n\
           g(() sync* {});\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`Iterable<dynamic> Function()` assigns into `Iterable<int> Function()`: silent"
    );
}

/// `async*` wraps through `Stream`: `d(() async* {})` against
/// `int Function()` reports (the oracle: `'Stream<dynamic> Function()'`),
/// and the `Stream<int>` destination twin stays silent.
#[test]
fn async_star_stream_wrap_decides() {
    let reported = report_ops(
        "class Stream<T> {}\n\
         void d(int Function() f) {}\n\
         void k() {\n\
           d(() async* {});\n\
         }\n",
    );
    assert_eq!(
        single(&reported),
        "argument_type_not_assignable",
        "`Stream<dynamic> Function()` rejects against `int Function()`: {:#?}",
        reported.diagnostics
    );
    let silent = report_ops(
        "class Stream<T> {}\n\
         void d(Stream<int> Function() f) {}\n\
         void k() {\n\
           d(() async* {});\n\
         }\n",
    );
    assert_eq!(
        codes(&silent),
        Vec::<&str>::new(),
        "`Stream<dynamic> Function()` assigns into `Stream<int> Function()`: silent"
    );
}

/// The async ENCLOSURE alone never silences: `g(() {})` inside an `async`
/// body reports the plain literal's arity deficit exactly as in a sync
/// body (the oracle reports both).
#[test]
fn async_enclosure_plain_literal_reports() {
    let folded = report_ops(
        "void g(void Function(dynamic) f) {}\n\
         Future<void> k() async {\n\
           g(() {});\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "async enclosure keeps the plain literal's row: {:#?}",
        folded.diagnostics
    );
}

/// The sync* enclosure twin: `g(() {})` inside a `sync*` body reports the
/// same way (the oracle reports both enclosures identically).
#[test]
fn sync_star_enclosure_plain_literal_reports() {
    let folded = report_ops(
        "void g(void Function(dynamic) f) {}\n\
         Iterable<int> k() sync* {\n\
           g(() {});\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "sync* enclosure keeps the plain literal's row: {:#?}",
        folded.diagnostics
    );
}

/// The initializer face reads the wrapped row: `int Function() h =
/// () async {};` reports `invalid_assignment` (the oracle rejects
/// `'Future<Null> Function()'` at the declaration), and the void twin
/// stays silent. The fixture declares its own `Future` head so the
/// wrapped face is a decidable rejection in the rig (the names-only
/// `dart:core` stub answers no subtype ask for the real head — the same
/// posture the #3061 fixtures pin).
#[test]
fn async_literal_initializer_rejects() {
    let reported = report_ops("class Future<T> {}\nint Function() h = () async {};\n");
    assert_eq!(
        single(&reported),
        "invalid_assignment",
        "the wrapped return rejects the int-return declaration: {:#?}",
        reported.diagnostics
    );
    let silent = report_ops("class Future<T> {}\nvoid Function() f = () async {};\n");
    assert_eq!(
        codes(&silent),
        Vec::<&str>::new(),
        "`Future<…> Function()` initializes the `void Function()` declaration: silent"
    );
}

/// The generator initializer twin: `int Function() f = () sync* {};`
/// reports `invalid_assignment` (the oracle rejects
/// `'Iterable<dynamic> Function()'`), and the `Iterable<int>` twin stays
/// silent.
#[test]
fn sync_star_literal_initializer_rejects() {
    let reported = report_ops("int Function() f = () sync* {};\n");
    assert_eq!(
        single(&reported),
        "invalid_assignment",
        "the generator wrap rejects the int-return declaration: {:#?}",
        reported.diagnostics
    );
    let silent = report_ops("Iterable<int> f = () sync* {};\n");
    assert_eq!(
        codes(&silent),
        Vec::<&str>::new(),
        "`Iterable<dynamic> Function()` initializes the `Iterable<int>` declaration: silent"
    );
}

/// A no-context untyped formal keeps `dynamic` and the capture lands:
/// `d((p) {})` against `T extends int` binds the parameter from the
/// literal's function shape and reports against the bound (the oracle:
/// `'Null Function(dynamic)' can't be assigned to 'int'`). The poison
/// silenced exactly this row.
#[test]
fn untyped_formal_no_context_generic_bound_reports() {
    let folded = report_ops(
        "void d<T extends int>(T x) {}\n\
         void k() {\n\
           d((p) {});\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "the no-context literal's signature constrains the bound parameter: {:#?}",
        folded.diagnostics
    );
}

/// The same capture against a non-function formal: `d((y) {})` against
/// `int x` reports (the oracle: `'Null Function(dynamic)' can't be
/// assigned to 'int'`) — a function value refuses every non-function
/// destination.
#[test]
fn untyped_formal_no_context_int_formal_reports() {
    let folded = report_ops(
        "void d(int x) {}\n\
         void k() {\n\
           d((y) {});\n\
         }\n",
    );
    assert_eq!(
        single(&folded),
        "argument_type_not_assignable",
        "the captured function shape refuses the int formal: {:#?}",
        folded.diagnostics
    );
}

/// Against a `dynamic` formal the capture stays silent — the oracle
/// answers nothing for `h((x) {})` over `h(g)` (dynamic accepts the
/// function value), so the un-poisoned capture must not over-fire.
#[test]
fn untyped_formal_no_context_dynamic_formal_stays_silent() {
    let folded = report_ops(
        "void h(g) {}\n\
         void k() {\n\
           h((x) {});\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`dynamic` accepts the literal's function shape: silent"
    );
}
