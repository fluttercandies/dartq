// Minimal local stand-in for package:meta (BSD-3, pub.dev/packages/meta):
// only the `sealed` and `literal` annotation faces, mirrored from meta
// 1.19.0 so the corpus resolves `package:meta/meta.dart` offline. The
// analyzer matches @sealed and @literal by a top-level getter of the same
// name in the library named `meta` (_isPackageMetaGetter in analyzer
// element.dart) — the `library meta;` directive and the getter names below
// are both load-bearing.
//
// The real declarations also carry @Target entries from meta_meta.dart;
// omitted here because every corpus application targets a kind the real
// target sets permit, so behavior is identical.
library meta;

/// Annotation marking a class as sealed: subclasses outside the declaring
/// package are static warnings (mixin_on_sealed_class for a direct `on`
/// constraint, subtype_of_sealed_class otherwise).
const Object sealed = _Sealed();

class _Sealed {
  const _Sealed();
}

/// Annotation marking a const constructor whose invocations must be
/// `const` (non_const_call_to_literal_constructor for a non-const,
/// const-evaluatable invocation).
const Object literal = _Literal();

class _Literal {
  const _Literal();
}
