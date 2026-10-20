//! The SDK extension-member faces: bundled declaration source for the
//! `dart:*` extension declarations whose members apply implicitly to
//! receiver types.
//!
//! [`crate::sdk_faces_gen::SDK_LIBRARY_FACES`] is a per-library top-level
//! name list — the import-namespace shape. Extension members (`toJS` on
//! `String`, …) surface on receiver types, not in import namespaces: the
//! semantic member walk resolves them through the element index's
//! extension-application pool (`FileElements::extension_faces`, reached via
//! the import edge's export scope → `index.get(declaring_file)`), and the
//! synthetic `<dart:…>` face files carry no element model — so a
//! `someString.toJS` read over-fired `undefined_getter` (the corpus's
//! `dart:js_interop` uses, #2186 S5).
//!
//! This table carries the missing faces as checked-in Dart source: the
//! pipeline scans + collects each excerpt once per analyze (a fixed
//! ~15-line parse, the same one-time shape as the graph's SDK node
//! registration) and indexes the result under the library's face file id —
//! the exact entries a real parsed unit would contribute, through the
//! existing extension-pool route, with no semantic-side change. The
//! extension names already stand in the generated top-level face table
//! (`sdk_faces_gen::SDK_LIBRARY_FACES`, the generator's full export
//! surface), so the graph's export scope exposes each face under its
//! declaring name — the pool's `face.name == exported name` filter.
//!
//! Source fidelity: verbatim member surfaces from the upstream Dart SDK
//! source — `sdk/lib/js_interop/js_interop.dart` and
//! `sdk/lib/js_interop_unsafe/js_interop_unsafe.dart` (stable across the
//! 3.13.1 oracle of record and 3.15). The `@Since('3.14')` list-conversion
//! extensions are absent there and stay out — the oracle does not resolve
//! them. `NumToJSExtension.toJS` is a concrete getter upstream; the excerpt
//! declares it `external` — the name-level model (name, kind, return type)
//! is identical and no body is captured either way.

use std::path::Path;

use dartq_language::FeatureSet;
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, scan};
use dartq_source::{FileTable, StringTable};

use dartq_semantic::{ElementIndex, collect};

/// Per-library bundled extension-face source: `(dart: library name, the
/// declaration excerpt)`. Only libraries whose corpus-visible surface
/// includes implicit-application members join; the table is empty of
/// libraries whose face is name-only complete.
static SDK_EXTENSION_FACE_SOURCES: &[(&str, &str)] = &[
    (
        "js_interop",
        r#"extension ListToJSArray<T extends JSAny?> on List<T> {
  external JSArray<T> get toJS;
  external JSArray<T> get toJSProxyOrRef;
}
extension DoubleToJSNumber on double {
  external JSNumber get toJS;
}
extension NumToJSExtension on num {
  external JSNumber get toJS;
}
extension BoolToJSBoolean on bool {
  external JSBoolean get toJS;
}
extension StringToJSString on String {
  external JSString get toJS;
}
"#,
    ),
    (
        // Upstream Dart SDK source, sdk/lib/js_interop_unsafe/
        // js_interop_unsafe.dart — the member surfaces verbatim. Concrete
        // shorthand bodies (`has` => hasProperty(...)) declare `external`:
        // the name-level model (name, kind, return type, formals) is
        // identical either way and no body is captured. The private
        // `_callMethod`/`_callAsConstructor` helpers stay out — they are
        // library-private upstream and invisible to every user-visible
        // resolution.
        "js_interop_unsafe",
        r#"extension JSObjectUnsafeUtilExtension on JSObject {
  external bool has(String property);
  external JSBoolean hasProperty(JSAny property);
  external JSAny? operator [](String property);
  external R getProperty<R extends JSAny?>(JSAny property);
  external void operator []=(String property, JSAny? value);
  external void setProperty(JSAny property, JSAny? value);
  external R callMethod<R extends JSAny?>(JSAny method, [JSAny? arg1, JSAny? arg2, JSAny? arg3, JSAny? arg4]);
  external R callMethodVarArgs<R extends JSAny?>(JSAny method, [List<JSAny?>? arguments]);
  external JSBoolean delete(JSAny property);
}
extension JSFunctionUnsafeUtilExtension on JSFunction {
  external R callAsConstructor<R>([JSAny? arg1, JSAny? arg2, JSAny? arg3, JSAny? arg4]);
  external R callAsConstructorVarArgs<R extends JSObject>([List<JSAny?>? arguments]);
}
"#,
    ),
];

/// Indexes every bundled excerpt's element model under its library's
/// synthetic face file — the id the graph build interns for
/// `<dart:{library}>` (same table, idempotent by path), so the semantic
/// extension pool finds the faces through the import edge's export scope.
///
/// Runs once per analyze, beside [`crate::semdiag::build_index`]; the
/// excerpts are fixed ASCII with no directives, so the scan is
/// config-stable (modern feature posture, the platform libraries' own) and
/// cannot produce scanner diagnostics.
pub(crate) fn insert_sdk_extension_faces(
    index: &mut ElementIndex,
    files: &mut FileTable,
    strings: &StringTable,
) {
    for (library, source) in SDK_EXTENSION_FACE_SOURCES {
        let file = files.intern(Path::new(&format!("<dart:{library}>")));
        let scanned = scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        debug_assert!(
            scanned.diagnostics.is_empty(),
            "bundled SDK extension source must scan clean"
        );
        let features = ParserFeatures::from_feature_set(FeatureSet::latest());
        let elements = collect(source, &scanned.tokens, features, strings);
        index.insert(file, elements);
    }
}
