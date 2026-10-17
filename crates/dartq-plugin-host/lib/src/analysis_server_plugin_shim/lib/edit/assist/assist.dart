/// The official `edit/assist/assist.dart`, mirrored as a pure re-export.
///
/// Mirrors `package:analysis_server_plugin/edit/assist/assist.dart`
/// (upstream Dart SDK source), which re-exports `AssistKind` from
/// `package:analyzer_plugin/utilities/assist/assist.dart` and adds the
/// `Assist` wrapper (kind + `SourceChange`). `AssistKind` is re-exported from
/// the same `package:analyzer_plugin` library the official package
/// re-exports — a plugin importing `AssistKind` from here and one importing
/// it directly from `package:analyzer_plugin` name the same declaration,
/// exactly as upstream. `Assist` wraps `package:analyzer_plugin`'s
/// `SourceChange`, an analyzer-plugin face the dartq wire v1 does not carry
/// (producers return the wire's edit list directly — see
/// `edit/dart/correction_producer.dart`), so it is not mirrored.
export 'package:analyzer_plugin/utilities/assist/assist.dart' show AssistKind;
