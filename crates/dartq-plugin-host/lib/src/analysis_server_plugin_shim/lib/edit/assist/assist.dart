/// The assist-kind vocabulary, mirrored verbatim.
///
/// Mirrors `package:analysis_server_plugin/edit/assist/assist.dart`
/// (upstream Dart SDK source), which re-exports `AssistKind` from
/// `package:analyzer_plugin/utilities/assist/assist.dart` and adds the
/// `Assist` wrapper (kind + `SourceChange`). `AssistKind` is pure data and is
/// mirrored exactly; `Assist` wraps `package:analyzer_plugin`'s
/// `SourceChange`, an analyzer-plugin face the dartq wire v1 does not carry
/// (producers return the wire's edit list directly — see
/// `edit/dart/correction_producer.dart`), so it is not mirrored.
final class AssistKind {
  /// The unique identifier of this kind of assist. May be used by client
  /// editors, for example to allow key-binding specific assists (or groups
  /// of).
  final String id;

  /// The priority of this kind of assist for the kind of error being
  /// addressed where a higher integer value indicates a higher priority and
  /// relevance.
  final int priority;

  /// A human-readable description of the changes that will be applied by
  /// this kind of assist. The message can contain parameters, where each
  /// parameter is represented by a zero-based index inside curly braces.
  final String message;

  /// Initialize a newly created kind of assist to have the given [id],
  /// [priority] and [message].
  const AssistKind(this.id, this.priority, this.message);

  @override
  String toString() => id;
}
