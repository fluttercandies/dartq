/// The correction-producer surface, mirrored onto the dartq wire v1.
///
/// Mirrors the names and shapes of
/// `package:analysis_server_plugin/edit/dart/correction_producer.dart`
/// (`.third/dart-sdk`) that the dartq wire v1 can express, and delegates them
/// onto the mirror's edit types.
///
/// The structural boundary, anchored: the official producer computes inside a
/// resolved analysis — `CorrectionProducer<T extends ParsedUnitResult>`,
/// `Future<void> compute(ChangeBuilder builder)`, the AST/element members
/// (`node`, `unit`, `utils`, `coveringNode`, the `ResolvedCorrectionProducer`
/// and `ParsedCorrectionProducer` bases, `MultiCorrectionProducer`). Those
/// need `package:analyzer`'s AST and results, which the dartq wire v1 does
/// not carry (the Query API, PRD section 14.2, lands that channel). A plugin
/// subclassing those official bases does not compile against this shim —
/// the boundary is at compile time, not runtime. What the wire CAN express is
/// a producer computing UTF-16 range edits from the request context, and that
/// is exactly what this file mirrors: same class names, same kind getters,
/// same per-request construction, with [CorrectionProducer.compute] returning
/// the wire's edit list.
library;

import 'package:dartq_plugin_host/dartq_plugin_host.dart' show SourceEdit;

export 'package:dartq_plugin_host/dartq_plugin_host.dart' show SourceEdit;

import '../assist/assist.dart' show AssistKind;
import '../fix/fix.dart' show FixKind;

/// How broadly a [CorrectionProducer] can be applied. Verbatim the official
/// enum (`CorrectionApplicability`), including the cumulative semantics
/// documented there; the dartq wire v1 reads none of it yet (bulk and
/// automatic application land with the Query API, PRD section 14.2), but the
/// values are part of the official authoring API this shim mirrors.
enum CorrectionApplicability {
  /// Indicates a correction can be applied only at a specific location.
  singleLocation,

  /// Indicates a correction can be applied in multiple positions in the same
  /// file.
  acrossSingleFile,

  /// Indicates a correction can be applied across in bulk across multiple
  /// files and/or at the same time as applying fixes from other producers.
  acrossFiles,

  /// Indicates a correction can be applied in multiple locations, even if not
  /// chosen explicitly as a tool action, and can be applied to potentially
  /// incomplete code.
  automatically,

  /// Indicates a correction can be applied in multiple files, except only one
  /// location per file.
  automaticallyButOncePerFile,
}

/// Everything the wire v1 gives a producer for one request: the UTF-16
/// selection the request (or, for fixes, the matched engine diagnostic)
/// anchors at.
///
/// Official counterpart: `CorrectionProducerContext` carries the parsed or
/// resolved unit, the covering node, and the token
/// (`lib/edit/dart/correction_producer.dart`). Those are analyzer faces the
/// wire v1 cannot supply; the shim carries the wire's selection under the
/// official member names.
final class CorrectionProducerContext {
  /// Builds the context for one request; [offset] and [length] are UTF-16
  /// units.
  const CorrectionProducerContext({
    required this.file,
    required this.offset,
    required this.length,
  });

  /// Path of the file the request targets.
  final String file;

  /// UTF-16 offset the request targets.
  final int offset;

  /// UTF-16 length the request targets.
  final int length;

  /// The path of the file being corrected (the official member name).
  String get path => file;

  /// The offset of the selection.
  int get selectionOffset => offset;

  /// The length of the selection.
  int get selectionLength => length;

  /// The end of the selection.
  int get selectionEnd => offset + length;
}

/// A stub [CorrectionProducerContext], verbatim the official pattern
/// (`StubCorrectionProducerContext`): used to instantiate a producer for the
/// purpose of examining its fixed properties (the kind getters) at
/// registration time. Every member access throws.
final class StubCorrectionProducerContext
    implements CorrectionProducerContext {
  /// The singleton stub instance.
  static final instance = StubCorrectionProducerContext._();

  StubCorrectionProducerContext._();

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('Unimplemented: ${invocation.memberName}');
}

/// A function that can be executed to create a [CorrectionProducer], verbatim
/// the official shape: the constructor tearoff of a producer subclass
/// (`SubProducer.new`).
typedef ProducerGenerator =
    CorrectionProducer Function({required CorrectionProducerContext context});

/// An object that can compute a correction (fix or assist), mirroring the
/// wire-expressible members of the official `CorrectionProducer`: the kind
/// getters the registration path reads, the selection members the official
/// base exposes, and per-request construction via a
/// `{required super.context}` constructor.
abstract class CorrectionProducer {
  /// Builds the producer for one request; subclasses take
  /// `SubProducer({required super.context})`.
  CorrectionProducer({required CorrectionProducerContext context})
    : _context = context;

  final CorrectionProducerContext _context;

  /// The applicability of this producer.
  CorrectionApplicability get applicability;

  /// The assist kind that should be used to build an assist, or null if this
  /// producer doesn't support assists.
  AssistKind? get assistKind => null;

  /// The fix kind that should be used to build a fix, or null if this
  /// producer doesn't support fixes.
  FixKind? get fixKind => null;

  /// The path of the file the correction is being produced in.
  String get file => _context.path;

  /// The offset of the selection.
  int get selectionOffset => _context.selectionOffset;

  /// The length of the selection.
  int get selectionLength => _context.selectionLength;

  /// The end of the selection.
  int get selectionEnd => _context.selectionEnd;

  /// Computes the edits for this producer over the selection the producer was
  /// constructed with.
  ///
  /// Official counterpart: `Future<void> compute(ChangeBuilder builder)` —
  /// the ChangeBuilder face needs the analyzer's change-building stack and is
  /// a documented boundary of this shim; the wire v1 face returns the edit
  /// list directly.
  Future<List<SourceEdit>> compute();
}
