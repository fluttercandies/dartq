/// The official `edit/fix/fix.dart`, mirrored as a pure re-export.
///
/// Mirrors `package:analysis_server_plugin/edit/fix/fix.dart`
/// (upstream Dart SDK source), which re-exports `FixKind` from
/// `package:analyzer_plugin/utilities/fixes/fixes.dart` and adds the `Fix`
/// wrapper (kind + `SourceChange`). `FixKind` is re-exported from the same
/// `package:analyzer_plugin` library the official package re-exports — a
/// plugin importing `FixKind` from here and one importing it directly from
/// `package:analyzer_plugin` name the same declaration, exactly as upstream.
/// `Fix` wraps `package:analyzer_plugin`'s `SourceChange`, an
/// analyzer-plugin face the dartq wire v1 does not carry (producers return
/// the wire's edit list directly — see `edit/dart/correction_producer.dart`),
/// so it is not mirrored.
export 'package:analyzer_plugin/utilities/fixes/fixes.dart' show FixKind;
