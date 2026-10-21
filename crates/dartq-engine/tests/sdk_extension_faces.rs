//! The SDK extension-face activation pins (#2488): the bundled
//! `<dart:core>` enum pair (`EnumName`/`EnumByName`, the #2476 census
//! face) and the `dart:js_interop` extension-type containers (the G11
//! face, #2477) reach the semantic extension pool / member walk through
//! the engine's indexed excerpts, with oracle-parity rows in both
//! directions — a face-backed member resolves silently, a genuinely
//! missing one still reports, and an applicability violation (`byName`
//! on an unbounded type parameter) keeps firing.

mod common;

use std::path::Path;

use common::Fixture;
use dartq_diagnostics::Diagnostic;
use dartq_engine::{CancellationToken, Engine};

/// The #2476 census rows: concrete `values.byName` and the method type
/// parameter bound by `extends Enum` both resolve the `EnumByName` face —
/// the rows the pre-activation posture over-fired. (The unbounded
/// violation lives in its own witness test: `codes` is per-file, so a
/// firing control row would not be separable here.)
const ENUM_FACE: &str = r#"enum Color { red, green, blue }

void concreteValues() {
  final c = Color.values.byName('red');
  print(c);
}

T pick<T extends Enum>(List<T> values, String name) => values.byName(name);

void use() {
  pick<Color>(Color.values, 'red');
}
"#;

/// The G11 face without `dart:js_interop_unsafe`: a `JSObject` receiver
/// resolves its container through the excerpt's extension-type
/// declarations, so member/operator misses fire (the pre-activation
/// posture read the receiver as unresolvable and stayed silent).
const JS_INTEROP_BARE: &str = r#"import 'dart:js_interop';

void f(JSObject o) {
  o['x'];
  o.callMethod('g');
  o.nope;
}
"#;

/// The same receiver with the unsafe import: the `JSObjectUnsafeUtilExtension`
/// faces apply (`[]`, `callMethod`) and the accesses resolve — only the
/// genuinely missing `nope` keeps reporting (the faces gate the specific
/// names they declare, never blanket-suppress the member face). The
/// argument rides `'g'.toJS` (the argument-type face is a separate
/// family, not this pin's contract).
const JS_INTEROP_UNSAFE: &str = r#"import 'dart:js_interop';
import 'dart:js_interop_unsafe';

void g(JSObject o) {
  o['x'];
  o.callMethod('g'.toJS);
  o.nope;
}
"#;

/// The deduped, sorted diagnostic codes reported for the file whose path
/// ends with `tail`.
fn codes(engine: &Engine, diagnostics: &[Diagnostic], tail: &str) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = diagnostics
        .iter()
        .filter(|diagnostic| {
            engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path: &Path| path.ends_with(tail))
        })
        .map(|diagnostic| diagnostic.code.name)
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

fn analyze(files: &[(&str, &str)]) -> (Engine, Vec<Diagnostic>) {
    let fixture = Fixture::new(files);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    (engine, diagnostics)
}

#[test]
fn bounded_enum_by_name_resolves() {
    let (engine, diagnostics) = analyze(&[("lib/main.dart", ENUM_FACE)]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        Vec::<&str>::new(),
        "the bounded `List<T extends Enum>` face must resolve silently"
    );
}

/// The oracle-parity control the #2488 activation exposed as a
/// diagnose-side gap, closed by #2497: `pickUnbounded`'s `List<T>`
/// receiver has no `T extends Enum` bound, the oracle rejects
/// `EnumByName`'s application and reports `undefined_method` (probed:
/// Dart SDK 3.13.1, the `pickUnbounded` row of the #2488 probe rig).
/// `row_resolution`'s extension applicability now checks the applied
/// face's type-parameter bound — an in-scope parameter argument answers
/// through its own declared bound, so the unbounded `T` reads `Object?`
/// and the `Enum` bound rejects it.
#[test]
fn unbounded_type_argument_violates_the_by_name_face() {
    let (engine, diagnostics) = analyze(&[(
        "lib/main.dart",
        "T pickUnbounded<T>(List<T> values, String name) => values.byName(name);\n",
    )]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        vec!["undefined_method"],
        "an unbounded type argument must keep violating the byName face"
    );
}

#[test]
fn js_object_member_misses_fire_without_the_unsafe_import() {
    let (engine, diagnostics) = analyze(&[("lib/main.dart", JS_INTEROP_BARE)]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        vec!["undefined_getter", "undefined_method", "undefined_operator"],
        "the JSObject container must back the member walk so misses fire"
    );
}

#[test]
fn unsafe_import_resolves_the_js_object_faces_and_keeps_genuine_misses() {
    let (engine, diagnostics) = analyze(&[("lib/main.dart", JS_INTEROP_UNSAFE)]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        vec!["undefined_getter"],
        "`[]` and `callMethod` resolve through the unsafe faces; the \
         undeclared `nope` keeps reporting"
    );
}

/// The #2502 pin: the receiver-instantiation environment pairs the
/// receiver's written type arguments with the container's DECLARED
/// parameter order — `Map<E2, String>`'s `putIfAbsent(K key, V
/// ifAbsent())` checks the key against `E2`, and `update`'s closure
/// parameter reads `String` (the pre-fix first-seen order collected the
/// return's `V` ahead of the key's `K` and swapped the pairing: the key
/// row read `String`, the closure parameter `E2`).
const MAP_MEMBER_ENV: &str = r#"class E2 { const E2(); }

void f(Map<E2, String> m, E2 e) {
  m[e] = 'x';
  m.putIfAbsent(e, () => 'x');
  m.update(e, (v) => v.toUpperCase());
  m.forEach((k, v) { print(v.length); });
  m.removeWhere((k, v) => v.isNotEmpty);
}
"#;

/// The control direction: a foreign-class key against the `E2` key
/// parameter fires on the oracle's row (the pre-fix swapped pairing
/// checked the key against `String` instead). The key spells a user class
/// — a builtin-headed source (`String`) against a user-class parameter is
/// a separate, pre-existing assignability under-report, not this pin's
/// contract.
const MAP_MEMBER_ENV_VIOLATION: &str = r#"class E2 { const E2(); }
class F { const F(); }

void f(Map<E2, String> m, F x) {
  m.putIfAbsent(x, () => 'x');
}
"#;

#[test]
fn map_member_receiver_instantiation_pairs_declaration_order() {
    let (engine, diagnostics) = analyze(&[("lib/main.dart", MAP_MEMBER_ENV)]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        Vec::<&str>::new(),
        "putIfAbsent/update must substitute K/V by declaration order"
    );
    let (engine, diagnostics) = analyze(&[("lib/main.dart", MAP_MEMBER_ENV_VIOLATION)]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        vec!["argument_type_not_assignable"],
        "a wrong-typed key must fire against the K=E2 parameter"
    );
}

/// The #2503 pin: dart:core's re-exported `IterableExtensions` family
/// (upstream sdk/lib/collection/iterable.dart, core.dart:182) resolves on
/// every iterable receiver — the null-aware getters return `T?` (the
/// `!= null` flow gate reads the real nullability), `elementAtOrNull` the
/// declared method face.
const ITERABLE_EXTENSIONS: &str = r#"class D {}

void f(List<D> ds) {
  final D? first = ds.firstOrNull;
  print(first);
  print(ds.lastOrNull);
  print(ds.singleOrNull);
  print(ds.elementAtOrNull(0));
  print(ds.indexed);
  print(ds.nonNulls);
  final D d = ds.firstOrNull!;
  print(d);
}
"#;

#[test]
fn iterable_extensions_family_resolves() {
    let (engine, diagnostics) = analyze(&[("lib/main.dart", ITERABLE_EXTENSIONS)]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        Vec::<&str>::new(),
        "the firstOrNull family must resolve through the core faces"
    );
}

/// The #2504 pin: a local `Iterable` declaration shadows dart:core's, so
/// the SDK's `EnumByName on Iterable` never applies to its receivers —
/// `byName` reports `undefined_method` exactly as the oracle does (the
/// auto-import resolves the receiver's head to the LOCAL declaration; the
/// pre-gate spelling-level applicability matched the bare name and added
/// a spurious ambiguity row beside the oracle's set).
const LOCAL_ITERABLE_SHADOW: &str = r#"class Iterable<T> {}

void f(Iterable<int> values) {
  values.byName('x');
}
"#;

#[test]
fn local_iterable_shadow_kills_the_sdk_enum_by_name_face() {
    let (engine, diagnostics) = analyze(&[("lib/main.dart", LOCAL_ITERABLE_SHADOW)]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        vec!["undefined_method"],
        "the SDK EnumByName face must not apply to a local Iterable head"
    );
}

/// The SDK-precedence pin: when a user extension twin-shapes an SDK face
/// — the same member on the same `on` type, here a package-style
/// `IterableExtension.firstOrNull` against dart:core's re-exported
/// `IterableExtensions.firstOrNull` — the oracle's specificity order
/// opens with provenance, not types
/// (`extension_member_resolver.dart:470-479`): a non-SDK extension is
/// unconditionally more specific than an SDK one, the user face silently
/// wins, and no ambiguity row fires (probed: Dart SDK 3.13.1 reports
/// "No issues found!" on this exact shape).
const SDK_PRECEDENCE_MAIN: &str =
    "import 'ext.dart';\n\nvoid f(List<int> xs) {\n  print(xs.firstOrNull);\n}\n";
const SDK_PRECEDENCE_EXT: &str =
    "extension IterableExtension<T> on Iterable<T> {\n  T? get firstOrNull => null;\n}\n";

#[test]
fn user_extension_twin_silently_wins_over_the_sdk_face() {
    let (engine, diagnostics) = analyze(&[
        ("lib/main.dart", SDK_PRECEDENCE_MAIN),
        ("lib/ext.dart", SDK_PRECEDENCE_EXT),
    ]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        Vec::<&str>::new(),
        "the user twin must silently beat the SDK face (no ambiguity row)"
    );
}

/// The nullability-absorption pin: `NullableIterableExtensions<T extends
/// Object> on Iterable<T?>` matches `Iterable<L?>` as `T := L` — the `on`
/// argument's own `?` absorbs the receiver argument's nullability BEFORE
/// the `T extends Object` bound runs, so every nullable-element iterable
/// keeps resolving `nonNulls` (the oracle: "No issues found!" on the
/// probe rig; the pre-fix posture read the bound against `L?` and
/// reported `undefined_getter`).
const NONNULLS_NULLABLE_ELEMENTS: &str = r#"class L {}

void g(List<L> xs, Iterable<L?> it, List<L?> ls) {
  print(xs.map((x) => x as L?).nonNulls);
  print(it.nonNulls);
  print(ls.nonNulls);
  print(xs.nonNulls);
}
"#;

#[test]
fn non_nulls_resolves_on_nullable_element_iterables() {
    let (engine, diagnostics) = analyze(&[("lib/main.dart", NONNULLS_NULLABLE_ELEMENTS)]);
    assert_eq!(
        codes(&engine, &diagnostics, "main.dart"),
        Vec::<&str>::new(),
        "the on-argument's `?` must absorb the receiver argument's `?` before the bound"
    );
}
