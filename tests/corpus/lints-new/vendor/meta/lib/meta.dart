// Minimal vendored package:meta (corpus #1160). Annotation shapes are
// transcribed from upstream Dart SDK source, pkg/meta/lib/meta.dart: the
// `immutable`/`literal` const instances and their annotation classes.
// Everything else in the real package is intentionally absent — the corpus
// references only these two faces.

/// Annotation marking a class as immutable (upstream meta.dart:649-655).
class Immutable {
  /// A human-readable explanation of the reason why the class is immutable.
  final String reason;

  /// Creates annotation for being immutable with the given [reason].
  const Immutable([this.reason = '']);
}

/// Annotation marking a constructor as invocable without the `const` keyword
/// (upstream meta.dart `Literal` face).
class Literal {
  /// A human-readable explanation of the annotation's purpose.
  final String comment;

  const Literal([this.comment = '']);
}

/// The canonical [Immutable] instance (upstream meta.dart `immutable`).
const Immutable immutable = Immutable();

/// The canonical [Literal] instance (upstream meta.dart `literal`).
const Literal literal = Literal();
