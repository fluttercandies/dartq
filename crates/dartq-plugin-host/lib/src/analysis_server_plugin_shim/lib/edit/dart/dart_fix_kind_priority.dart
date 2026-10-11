/// Fix-kind priority constants, verbatim the official
/// `package:analysis_server_plugin/edit/dart/dart_fix_kind_priority.dart`.
abstract final class DartFixKindPriority {
  /// The default priority for a quick fix.
  static const int standard = 50;

  /// The default priority for a quick fix that applies to the whole file.
  static const int inFile = 40;

  /// The default priority for a quick fix that ignores a diagnostic.
  static const int ignore = 30;
}
