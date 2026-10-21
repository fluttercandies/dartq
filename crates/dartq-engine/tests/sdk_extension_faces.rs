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
