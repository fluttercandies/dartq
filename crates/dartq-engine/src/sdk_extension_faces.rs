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
//!
//! The `js_interop` excerpt also carries the library's extension-type
//! declarations (`JSAny`, `JSObject`, `JSFunction` — upstream
//! js_interop.dart:181/:200/:248). Without a container for the receiver's
//! type spelling, the semantic member walk reads it as unresolvable
//! (`ChainBroken`): member and operator misses rode dynamic and stayed
//! silent where the oracle reports them. The declarations' internal
//! `JS*Type` representation spellings stay undeclared — they live in
//! `dart:_js_types` upstream, which `dart:js_interop` imports but never
//! exports, so no user-visible resolution can name them either.
//!
//! The table consumes two sources: this engine's built-in excerpts and
//! [`dartq_sdk::EXTENSION_FACE_SOURCES`] (the `dart:core` enum pair the
//! census keyed on — `values.byName` over `List<T extends Enum>`
//! receivers). Both flow through the same scan-collect-index shape below;
//! the tables must stay library-disjoint, since a duplicate entry would
//! silently overwrite the earlier face's element model (pinned by an
//! in-module test).

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
        // Upstream Dart SDK source, sdk/lib/js_interop/js_interop.dart —
        // the declaration surfaces verbatim. Concrete member bodies are
        // spelled `external` (the name-level model is identical and no
        // body is captured either way); the private `_getProperty`/
        // `_callMethod` members of `JSAny` and the private top-level
        // `_createObjectLiteral` stay out — library-private upstream,
        // invisible to every user-visible resolution. The `@JS`/`@Since`
        // annotations are dropped: metadata plays no part in the
        // resolution surfaces the faces carry.
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
// The nullable-receiver faces (upstream Dart SDK source,
// sdk/lib/js_interop/js_interop.dart:1021-1231): both extensions declare
// `on JSAny?` / `on Object?` — their members apply to a NULLABLE receiver
// as the safe match (the nullable-`on` application the extension walk
// already carries), so `payload.isUndefinedOrNull` over a `JSAny?`
// parameter resolves and never reports
// `unchecked_use_of_nullable_value`. Concrete bodies
// (`isUndefinedOrNull` => `this == null`, `instanceOfString`'s
// `globalContext` walk) spell `external` — the name-level model is
// identical and no body is captured either way.
extension NullableUndefineableJSAnyExtension on JSAny? {
  external bool get isUndefined;
  external bool get isNull;
  external bool get isUndefinedOrNull;
  external bool get isDefinedAndNotNull;
}
extension JSAnyUtilityExtension on JSAny? {
  external bool typeofEquals(String typeString);
  external bool instanceof(JSFunction constructor);
  external bool instanceOfString(String constructorName);
  external Object? dartify();
}
extension NullableObjectUtilExtension on Object? {
  external JSAny? jsify();
  external bool isA<T extends JSAny?>();
}
extension type JSAny._(JSAnyType _jsAny) implements Object, JSAnyType {}
extension type JSObject._(JSObjectType _jsObject)
    implements JSAny, JSObjectType {
  external JSObject.fromInteropObject(Object interopObject);
  external JSObject();
  external static JSObject? getPrototypeOf(JSAny value);
}
extension type JSFunction<T extends Function>._(JSFunctionType _jsFunction)
    implements JSObject, JSFunctionType {
  external int get length;
  external String get name;
}
// The primitive and array extension types (upstream :515/:822/:826/:830) —
// receiver spellings the cast face and the member walk resolve; the
// internal `JS*Type` representation spellings and the `JSIterable`
// protocol edge stay undeclared like `JSAnyType` above (upstream they
// live in `dart:_js_types`, which `dart:js_interop` imports but never
// exports — no user-visible resolution can name them either).
extension type JSArray<T extends JSAny?>._(JSArrayType _jsArray)
    implements JSObject, JSArrayType, JSIterable<T> {
  external JSArray();
  external JSArray.withLength(int length);
  external int get length;
  external set length(int newLength);
  external T operator [](int position);
  external void operator []=(int position, T value);
  external void add(T value);
}
extension type JSNumber._(JSNumberType _jsNumber)
    implements JSAny, JSNumberType {}
extension type JSBoolean._(JSBooleanType _jsBoolean)
    implements JSAny, JSBooleanType {}
extension type JSString._(JSStringType _jsString)
    implements JSAny, JSStringType {}
// The interop-to-Dart conversion extensions (upstream :2061/:2128/:2151/
// :2163) — `JSNumberToNumber` spells exactly `toDartDouble`/`toDartInt`
// (no plain `toDart` exists upstream for numbers).
extension JSArrayToList<T extends JSAny?> on JSArray<T> {
  external List<T> get toDart;
}
extension JSNumberToNumber on JSNumber {
  external double get toDartDouble;
  external int get toDartInt;
}
extension JSBooleanToBool on JSBoolean {
  external bool get toDart;
}
extension JSStringToString on JSString {
  external String get toDart;
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
    (
        // Upstream Dart SDK source, sdk/lib/ffi/ffi.dart:343-368 — the
        // `NativeFunctionPointer` extension surface verbatim. The
        // `@DartRepresentationOf` metadata plays no part in the resolution
        // surfaces the faces carry and is dropped; the concrete body
        // declares `external` (the name-level model is identical and no
        // body is captured either way). Without this face a
        // `p.asFunction<DF>()` read over a `Pointer<NativeFunction<NF>>`
        // receiver reports `undefined_method` instead of resolving — the
        // `leaf_call_must_not_*` / `non_native_function_type_argument_to_
        // pointer` corpus rows hang off exactly that send.
        "ffi",
        r#"extension NativeFunctionPointer<NF extends Function>
    on Pointer<NativeFunction<NF>> {
  external DF asFunction<DF extends Function>({bool isLeaf = false});
}
"#,
    ),
];

/// Indexes every bundled excerpt's element model under its library's
/// synthetic face file — the id the graph build interns for
/// `<dart:{library}>` (same table, idempotent by path), so the semantic
/// extension pool finds the faces through the import edge's export scope.
///
/// Both tables feed the same scan-collect-index shape: the engine's
/// built-in excerpts and the sdk crate's [`dartq_sdk::EXTENSION_FACE_SOURCES`]
/// (the `dart:core` enum pair). Runs once per analyze, beside
/// [`crate::semdiag::build_index`]; the excerpts are fixed ASCII with no
/// directives, so the scan is config-stable (modern feature posture, the
/// platform libraries' own) and cannot produce scanner diagnostics.
pub(crate) fn insert_sdk_extension_faces(
    index: &mut ElementIndex,
    files: &mut FileTable,
    strings: &StringTable,
) {
    for (library, source) in SDK_EXTENSION_FACE_SOURCES
        .iter()
        .chain(dartq_sdk::EXTENSION_FACE_SOURCES.iter())
    {
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

#[cfg(test)]
mod tests {
    use super::SDK_EXTENSION_FACE_SOURCES;
    use dartq_sdk::{EXTENSION_FACE_SOURCES, FLAT_LIBRARIES};

    /// The two face-source tables feed one loop, and the loop indexes each
    /// excerpt under its library's face file with whole-file replacement —
    /// a library carried by both tables would silently drop the earlier
    /// face's element model. The tables are curated static data in two
    /// crates, so the disjointness contract is pinned where both are
    /// visible, and every entry must name an addressable `dart:` library
    /// (one the generated flat surface knows, so the synthetic face file
    /// pairs with a library the graph build interned).
    #[test]
    fn chained_face_tables_are_disjoint_and_addressable() {
        let mut seen = std::collections::BTreeSet::new();
        for (library, source) in SDK_EXTENSION_FACE_SOURCES
            .iter()
            .chain(EXTENSION_FACE_SOURCES.iter())
        {
            assert!(
                seen.insert(*library),
                "extension face `{library}` carried by both tables: the later \
                 index.insert would silently overwrite the earlier face"
            );
            assert!(
                FLAT_LIBRARIES.iter().any(|(name, _)| name == library),
                "extension face `{library}` is not an addressable dart: library"
            );
            assert!(!source.is_empty(), "extension face `{library}` is empty");
        }
    }

    /// The `dart:ffi` excerpt carries the `NativeFunctionPointer`
    /// extension's `asFunction` accessor (upstream Dart SDK source,
    /// `sdk/lib/ffi/ffi.dart`): the member resolution for
    /// `p.asFunction<...>()` runs through the generated `ffi` face list,
    /// so both the extension name there and the excerpt's member spelling
    /// must stand — a dropped face resurfaces as `undefined_method`.
    #[test]
    fn ffi_face_carries_native_function_pointer_as_function() {
        let (_, source) = SDK_EXTENSION_FACE_SOURCES
            .iter()
            .find(|(library, _)| *library == "ffi")
            .expect("dart:ffi extension face present");
        assert!(
            source.contains("extension NativeFunctionPointer"),
            "excerpt lost the NativeFunctionPointer extension: {source}"
        );
        assert!(
            source.contains("asFunction"),
            "excerpt lost the asFunction accessor: {source}"
        );
        let (_, faces) = crate::sdk_faces_gen::SDK_LIBRARY_FACES
            .iter()
            .find(|(name, _)| *name == "ffi")
            .expect("generated ffi face list");
        assert!(
            faces.contains(&"NativeFunctionPointer"),
            "generated ffi face list lost NativeFunctionPointer"
        );
    }
}
