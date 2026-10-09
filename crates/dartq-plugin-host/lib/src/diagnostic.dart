/// Diagnostics a plugin emits over `notification/diagnostic`.
///
/// Mirrors `PluginDiagnostic` / `Severity` in the Rust crate `dartq-plugin`
/// (`src/session.rs`); both files must change together.
library;

/// Severity vocabulary of plugin diagnostics; the enum names are the wire
/// names.
enum Severity {
  /// An error diagnostic.
  error,

  /// A warning diagnostic.
  warning,

  /// An informational diagnostic.
  info,
}

/// One diagnostic located in one file.
class PluginDiagnostic {
  /// Builds a diagnostic; [offset] and [length] are UTF-16 units and default
  /// to zero-width.
  const PluginDiagnostic({
    required this.file,
    required this.severity,
    required this.code,
    required this.message,
    this.offset = 0,
    this.length = 0,
  });

  /// Path of the file the diagnostic refers to.
  final String file;

  /// Diagnostic severity.
  final Severity severity;

  /// The plugin's diagnostic code, e.g. `my_plugin/avoid_sync_io`.
  final String code;

  /// Human-readable message.
  final String message;

  /// UTF-16 offset of the highlighted range.
  final int offset;

  /// Length of the highlighted range in UTF-16 units.
  final int length;

  /// The diagnostic as the notification frame's `params` object.
  Map<String, Object?> toJson() => {
    'file': file,
    'severity': severity.name,
    'code': code,
    'message': message,
    'offset': offset,
    'length': length,
  };
}
