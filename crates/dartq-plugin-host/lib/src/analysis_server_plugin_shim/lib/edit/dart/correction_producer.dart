/// The correction-producer surface, mirrored onto the dartq wire v1.
///
/// Mirrors `package:analysis_server_plugin/edit/dart/correction_producer.dart`
/// (upstream Dart SDK source) as ONE hierarchy with the official names,
/// signatures, and constructor shape (`Sub({required super.context})`):
/// [CorrectionProducer] carries the shared member set (kinds, selection,
/// applicability, the AST/resolution faces), and [ParsedCorrectionProducer] /
/// [ResolvedCorrectionProducer] / [CorrectionProducerWithDiagnostic] sit under
/// it exactly as upstream — [ResolvedCorrectionProducer] is generic over
/// `ResolvedUnitResult`, which `implements ParsedUnitResult` (analyzer 14.5.0
/// line), so the official `ProducerGenerator` shape (which is what this
/// shim's `ProducerGenerator` is) accepts every producer family's constructor
/// tearoff naturally.
///
/// Shim deltas, each anchored:
/// - `compute` has the official signature — `Future<void>
///   compute(ChangeBuilder builder)` over the real `package:analyzer_plugin`
///   ChangeBuilder. The wire's edit ACQUISITION is not part of the producer
///   face: the registry adapter drives a producer's compute and harvests the
///   builder's `sourceChange`, the way the official fix processor does
///   (`pkg/analysis_server_plugin/lib/src/correction/fix_processor.dart`).
///   Dart-file edits need the analysis session the wire does not carry, so a
///   producer editing a Dart file through the builder fails contained until
///   the engine Query API lands that channel.
/// - The members owned by the analyzer's AST/resolution faces (`node`,
///   `unit`, `utils`, `typeProvider`, ...) exist with the official types and
///   throw a typed error when reached — the dartq wire never carries an AST;
///   the engine Query API lands that channel.
/// - [CorrectionProducer] is not `sealed`: the in-package fixture plugins
///   extend it directly (upstream reserves the sealed base for its own
///   subtypes).
library;

import 'dart:io' show Platform;

import 'package:analyzer/dart/analysis/results.dart';
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/dart/ast/token.dart';
import 'package:analyzer/dart/element/element.dart';
import 'package:analyzer/dart/element/type_provider.dart';
import 'package:analyzer/dart/element/type_system.dart';
import 'package:analyzer/diagnostic/diagnostic.dart';
import 'package:analyzer/file_system/file_system.dart' show ResourceProvider;
import 'package:analyzer/file_system/physical_file_system.dart'
    show PhysicalResourceProvider;
import 'package:analyzer/source/source_range.dart' show SourceRange;
import 'package:analyzer/src/dart/analysis/session_helper.dart'
    show AnalysisSessionHelper;
import 'package:analyzer_plugin/utilities/change_builder/change_builder_core.dart'
    show ChangeBuilder;

import '../assist/assist.dart' show AssistKind;
import '../correction_utils.dart' show CorrectionUtils;
import '../fix/fix.dart' show FixKind;

/// How broadly a producer can be applied. Verbatim the official enum
/// (`CorrectionApplicability`), including the cumulative semantics
/// documented there; the dartq wire v1 reads none of it yet (bulk and
/// automatic application land with the engine Query API), but the
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
/// anchors at, and the file path.
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
    required this.path,
    required int offset,
    required int length,
  }) : _selectionOffset = offset,
       _selectionLength = length;

  /// Path of the file the request targets.
  final String path;

  /// UTF-16 offset the request targets.
  final int _selectionOffset;

  /// UTF-16 length the request targets.
  final int _selectionLength;

  /// The end of the selection.
  int get _selectionEnd => _selectionOffset + _selectionLength;

  /// The diagnostic being fixed, or null for an assist. The wire v1 carries
  /// only the diagnostic's range (it is the selection above), never a
  /// [Diagnostic] object, so this is always null — the official `null`
  /// assist value, extended to fixes.
  Diagnostic? get diagnostic => null;

  /// The most deeply nested node covering the selection. The wire v1 carries
  /// no AST; accessing it throws the typed boundary error (the engine Query
  /// API lands this channel).
  AstNode get node => throw UnsupportedError(_boundaryMessage('node'));

  static String _boundaryMessage(String member) =>
      'package:analysis_server_plugin shim: `$member` needs the analyzer '
      'AST/resolution faces the dartq wire does not carry; it lands with '
      'the engine Query API.';
}

/// A stub [CorrectionProducerContext], verbatim the official pattern
/// (`StubCorrectionProducerContext`): used to instantiate a producer for the
/// purpose of examining its fixed properties (the kind getters) at
/// registration time. Every member access throws.
final class StubCorrectionProducerContext implements CorrectionProducerContext {
  /// The singleton stub instance.
  static final instance = StubCorrectionProducerContext._();

  StubCorrectionProducerContext._();

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('Unimplemented: ${invocation.memberName}');
}

/// A function that can be executed to create a correction producer, verbatim
/// the official shape: the constructor tearoff of a producer subclass
/// (`SubProducer.new`). [ParsedCorrectionProducer] and
/// [ResolvedCorrectionProducer] are both subtypes of
/// [CorrectionProducer]<[ParsedUnitResult]> — the resolved one through
/// `ResolvedUnitResult implements ParsedUnitResult` — so one generator type
/// accepts both families' tearoffs, exactly as upstream.
typedef ProducerGenerator =
    CorrectionProducer<ParsedUnitResult> Function({
      required CorrectionProducerContext context,
    });

/// An object that can compute a correction (fix or assist) in a Dart file.
///
/// Verbatim the official base (`_AbstractCorrectionProducer` plus the public
/// `CorrectionProducer<T>` generic face, with the official member set merged
/// onto one class): kinds, selection, applicability, the AST/resolution
/// faces, and the official `compute`. The dartq deltas are the class doc's
/// three anchored points — the edit acquisition lives in the registry
/// adapter, the AST/resolution members throw the typed boundary error, and
/// the class is not `sealed` (the in-package fixture producers extend it).
abstract class CorrectionProducer<T extends ParsedUnitResult> {
  /// Builds the producer for one request; subclasses take
  /// `SubProducer({required super.context})`.
  CorrectionProducer({required CorrectionProducerContext context})
    : _context = context;

  final CorrectionProducerContext _context;

  Never _unimplemented(String member) => throw UnsupportedError(
    'package:analysis_server_plugin shim: `$member` needs the analyzer '
    'AST/resolution faces the dartq wire does not carry; it lands with the '
    'engine Query API.',
  );

  /// The applicability of this producer.
  ///
  /// This property is to be implemented by each subclass, but outside code
  /// uses the `canBeApplied*` properties to determine a producer's
  /// applicability.
  CorrectionApplicability get applicability;

  /// The arguments that should be used when composing the message for an
  /// assist, or null if the assist message has no parameters or if this
  /// producer doesn't support assists.
  List<String>? get assistArguments => null;

  /// The assist kind that should be used to build an assist, or null if this
  /// producer doesn't support assists.
  AssistKind? get assistKind => null;

  /// Whether this producer can be used to apply a correction in multiple
  /// positions simultaneously in bulk across multiple files and/or at the
  /// same time as applying corrections from other producers.
  bool get canBeAppliedAcrossFiles =>
      applicability == CorrectionApplicability.acrossFiles ||
      applicability == CorrectionApplicability.automatically ||
      applicability == CorrectionApplicability.automaticallyButOncePerFile;

  /// Whether this producer can be used to apply a correction in multiple
  /// positions simultaneously across a file.
  bool get canBeAppliedAcrossSingleFile =>
      applicability == CorrectionApplicability.acrossSingleFile ||
      applicability == CorrectionApplicability.acrossFiles ||
      applicability == CorrectionApplicability.automatically;

  /// Whether this producer can be used to apply a correction automatically
  /// when code could be incomplete, as well as in multiple positions
  /// simultaneously in bulk across multiple files and/or at the same time as
  /// applying corrections from other producers.
  bool get canBeAppliedAutomatically =>
      applicability == CorrectionApplicability.automatically ||
      applicability == CorrectionApplicability.automaticallyButOncePerFile;

  /// The most deeply nested node that completely covers the highlight region
  /// of the diagnostic, or null if there is no diagnostic or if such a node
  /// does not exist.
  ///
  /// The wire v1 never carries a [Diagnostic], so this always answers null
  /// (the official no-diagnostic branch); the AST-backed branch beyond it is
  /// the boundary.
  AstNode? get coveringNode {
    if (_coveringNodeIsSet) {
      return _coveringNode;
    }

    _coveringNodeIsSet = true;
    final diagnostic = this.diagnostic;
    if (diagnostic == null) {
      return null;
    }
    throw _unimplemented('coveringNode');
  }

  AstNode? _coveringNode;

  bool _coveringNodeIsSet = false;

  /// The default EOL to be used for new files and files that do not have
  /// EOLs. Official counterpart: `utils.endOfLine` (which itself falls back
  /// to the platform line terminator); the wire v1 carries no file content,
  /// so the fallback applies.
  String get defaultEol => Platform.lineTerminator;

  /// The length of the source range associated with the diagnostic being
  /// fixed, or null if there is no diagnostic.
  int? get diagnosticLength => diagnostic?.problemMessage.length;

  /// The offset of the source range associated with the diagnostic being
  /// fixed, or null if there is no diagnostic.
  int? get diagnosticOffset => diagnostic?.problemMessage.offset;

  /// The diagnostic being fixed, or null if this producer is being used to
  /// produce an assist. The wire v1 carries only the diagnostic's range (the
  /// selection), so this is null on the wire.
  Diagnostic? get diagnostic => _context.diagnostic;

  @Deprecated("Use 'diagnosticLength' instead")
  int? get errorLength => diagnosticLength;

  @Deprecated("Use 'diagnosticOffset' instead")
  int? get errorOffset => diagnosticOffset;

  /// The arguments that should be used when composing the message for a fix,
  /// or null if the fix message has no parameters or if this producer
  /// doesn't support fixes.
  List<String>? get fixArguments => null;

  /// The fix kind that should be used to build a fix, or null if this
  /// producer doesn't support fixes.
  FixKind? get fixKind => null;

  /// The arguments that should be used when composing the message for a
  /// multi-fix, or null if the fix message has no parameters or if this
  /// producer doesn't support multi-fixes.
  List<String>? get multiFixArguments => null;

  /// The fix kind that should be used to build a multi-fix, or null if this
  /// producer doesn't support multi-fixes.
  FixKind? get multiFixKind => null;

  /// Whether the fixes are being built for the bulk-fix request. The wire v1
  /// has no bulk-fix requests.
  bool get applyingBulkFixes => false;

  /// The path of the file the correction is being produced in.
  String get file => _context.path;

  /// The parsed library result for the correction. The wire v1 carries no
  /// analysis session.
  ParsedLibraryResult get libraryResult => throw _unimplemented('libraryResult');

  /// The most deeply nested node that completely covers the selection. The
  /// wire v1 carries no AST.
  AstNode get node => _context.node;

  /// The resource provider of the sidecar process (the official counterpart
  /// reads it off the analysis session; for a dartq plugin process the
  /// physical provider is the file system the host analyzes).
  ResourceProvider get resourceProvider => PhysicalResourceProvider.INSTANCE;

  /// The end of the selection.
  int get selectionEnd => _context._selectionEnd;

  /// The length of the selection.
  int get selectionLength => _context._selectionLength;

  /// The offset of the selection.
  int get selectionOffset => _context._selectionOffset;

  /// The session helper of the analysis session. The wire v1 carries no
  /// analysis session.
  AnalysisSessionHelper get sessionHelper => throw _unimplemented(
    'sessionHelper',
  );

  /// The first token of the covering node. The wire v1 carries no AST.
  Token get token => throw _unimplemented('token');

  /// The compilation unit being corrected. The wire v1 carries no AST.
  CompilationUnit get unit => throw _unimplemented('unit');

  /// The parse result of the unit being corrected. The wire v1 carries no
  /// analysis results.
  ParsedUnitResult get unitResult => throw _unimplemented('unitResult');

  /// The line, prefix, and range-text helpers over the unit's content. The
  /// wire v1 carries no file content.
  CorrectionUtils get utils => throw _unimplemented('utils');

  /// Returns the text of the given [range] in the unit.
  String getRangeText(SourceRange range) => utils.getRangeText(range);

  /// Computes the changes for this producer using [builder].
  ///
  /// This method should not modify [fixKind].
  Future<void> compute(ChangeBuilder builder);
}

/// A correction producer that can work on non-resolved units
/// ([ParsedUnitResult]s), verbatim the official authoring base.
abstract class ParsedCorrectionProducer
    extends CorrectionProducer<ParsedUnitResult> {
  /// Builds the producer for one request; subclasses take
  /// `Sub({required super.context})`.
  ParsedCorrectionProducer({required super.context});
}

/// A correction producer that computes a correction (fix or assist) in a
/// Dart file, verbatim the official authoring base and its resolved-unit
/// members. The resolution-owned members exist with the official types and
/// throw the typed boundary error when reached (the wire v1 never resolves).
abstract class ResolvedCorrectionProducer
    extends CorrectionProducer<ResolvedUnitResult> {
  /// Builds the producer for one request; subclasses take
  /// `Sub({required super.context})`.
  ResolvedCorrectionProducer({required super.context});

  /// Whether [node] is in a static context. Needs the AST; the wire v1
  /// carries none.
  bool get inStaticContext => throw _unimplemented('inStaticContext');

  /// The library element for the library in which the correction is being
  /// produced. The wire v1 carries no resolution.
  LibraryElement get libraryElement2 => throw _unimplemented('libraryElement2');

  /// The type provider appropriate to the library in which the correction is
  /// requested. The wire v1 carries no resolution.
  TypeProvider get typeProvider => throw _unimplemented('typeProvider');

  /// The type system appropriate to the library in which the correction is
  /// requested. The wire v1 carries no resolution.
  TypeSystem get typeSystem => throw _unimplemented('typeSystem');

  @override
  ResolvedUnitResult get unitResult => throw _unimplemented('unitResult');
}

/// A resolved producer that asserts the diagnostic is present, verbatim the
/// official base: [diagnostic] is non-null by contract (on the dartq wire it
/// still answers null, so the assertion fires at the access site — fixes
/// whose compute reads `diagnostic` hit the wire boundary there).
abstract class CorrectionProducerWithDiagnostic
    extends ResolvedCorrectionProducer {
  /// Builds the producer for one request; subclasses take
  /// `Sub({required super.context})`.
  CorrectionProducerWithDiagnostic({required super.context});

  @override
  Diagnostic get diagnostic => super.diagnostic!;
}
