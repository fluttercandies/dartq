// Minimal local stand-in for package:meta (BSD-3, pub.dev/packages/meta):
// only the `sealed` annotation face, mirrored from meta 1.19.0 so the corpus
// resolves `package:meta/meta.dart` offline. The analyzer matches @sealed by
// a top-level getter named `sealed` in the library named `meta`
// (_isPackageMetaGetter in analyzer element.dart) — the `library meta;`
// directive and the getter name below are both load-bearing.
//
// The real declaration also carries @Target({TargetKind.classType}) from
// meta_meta.dart; omitted here because every corpus application targets a
// class, which the real target set permits, so behavior is identical.
library meta;

/// Annotation marking a class as sealed: subclasses outside the declaring
/// package are static warnings (mixin_on_sealed_class for a direct `on`
/// constraint, subtype_of_sealed_class otherwise).
const Object sealed = _Sealed();

class _Sealed {
  const _Sealed();
}
