/// The per-cycle execution config the core pushes as
/// `notification/plugin_config`.
///
/// Mirrors `CycleConfig` in the Rust crate `dartq-plugin`
/// (`src/session.rs`); both files must change together. The empty config is
/// the official no-configuration posture: lint rules stay disabled, warning
/// rules stay enabled, declared severities stand, files read from disk.
library;

import 'diagnostic.dart';
import 'frame.dart';

/// One unsaved-text overlay: the core owns the open-document table and
/// pushes the client's text so the host parses the edited view, never stale
/// disk bytes. `content == null` marks a previously overlaid file whose text
/// now lives on disk again.
final class OverlayEntry {
  /// Builds an overlay entry from the wire object.
  const OverlayEntry({required this.file, required this.content});

  /// Path of the overlaid file (matching the analyzed set's path form).
  final String file;

  /// The unsaved text, or null to fall back to disk.
  final String? content;
}

/// The resolved per-cycle execution config: which of the plugin's census
/// rules are enabled, the per-code severity overrides, and the open-document
/// overlays.
final class PluginCycleConfig {
  /// The config the session serves before the first push (and whenever the
  /// core pushes nothing): the official empty configuration.
  const PluginCycleConfig({
    this.enabledRules = const <String>{},
    this.severityOverrides = const <String, Severity>{},
    this.overlays = const <OverlayEntry>[],
  });

  /// Parses the `notification/plugin_config` params. Every field is
  /// required on the wire; a wrong shape is a [PluginProtocolException] (the
  /// session tears down, matching the contract for any malformed payload).
  factory PluginCycleConfig.fromJson(Map<String, Object?> params) {
    final enabledRules = params['enabled_rules'];
    if (enabledRules is! List) {
      throw const PluginProtocolException(
        'plugin_config payload requires an `enabled_rules` array',
      );
    }
    final overrides = params['severity_overrides'];
    if (overrides is! Map) {
      throw const PluginProtocolException(
        'plugin_config payload requires a `severity_overrides` object',
      );
    }
    final overlays = params['overlays'];
    if (overlays is! List) {
      throw const PluginProtocolException(
        'plugin_config payload requires an `overlays` array',
      );
    }
    final rules = <String>{
      for (final name in enabledRules)
        if (name is String)
          name.toLowerCase()
        else
          throw const PluginProtocolException(
            'plugin_config `enabled_rules` entries must be strings',
          ),
    };
    final severities = <String, Severity>{};
    overrides.forEach((code, severity) {
      if (code is! String) {
        throw const PluginProtocolException(
          'plugin_config `severity_overrides` keys must be strings',
        );
      }
      if (severity is! String) {
        throw PluginProtocolException(
          'plugin_config severity for `$code` must be a string',
        );
      }
      severities[code] = severityByName(severity);
    });
    final overlayEntries = <OverlayEntry>[
      for (final overlay in overlays)
        if (overlay is Map)
          OverlayEntry(
            file: _stringField(overlay, 'file'),
            content: overlay['content'] is String
                ? overlay['content'] as String
                : null,
          )
        else
          throw const PluginProtocolException(
            'plugin_config `overlays` entries must be objects',
          ),
    ];
    return PluginCycleConfig(
      enabledRules: rules,
      severityOverrides: severities,
      overlays: overlayEntries,
    );
  }

  /// The enabled rule names (the plugin's own census names, lower-cased).
  final Set<String> enabledRules;

  /// Per-code severity overrides, keyed by lower-case code name.
  final Map<String, Severity> severityOverrides;

  /// The open-document overlays (dirty file → unsaved text).
  final List<OverlayEntry> overlays;

  /// The overlay text for [file], or null when the file reads from disk.
  String? overlayFor(String file) {
    for (final overlay in overlays) {
      if (overlay.file == file) {
        return overlay.content;
      }
    }
    return null;
  }

  static String _stringField(Map<Object?, Object?> map, String field) {
    final value = map[field];
    if (value is String) {
      return value;
    }
    throw PluginProtocolException(
      'plugin_config overlay field `$field` must be a string',
    );
  }
}

/// The wire severity for [name]; anything else is a contract break.
Severity severityByName(String name) => switch (name) {
  'error' => Severity.error,
  'warning' => Severity.warning,
  'info' => Severity.info,
  _ => throw PluginProtocolException(
    'severity must be error/warning/info, got `$name`',
  ),
};
