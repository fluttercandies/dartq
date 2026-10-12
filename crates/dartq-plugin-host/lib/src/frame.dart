/// Wire frames and the newline-delimited JSON codec of the dartq plugin
/// protocol (server side).
///
/// Mirrors `src/frame.rs` of the Rust crate `dartq-plugin` — the wire schema,
/// method names, and the 16 MiB frame bound are shared; both files must
/// change together. The full contract (method table, handshake sequence) is
/// specified in the Rust crate's documentation.
library;

import 'dart:convert';

/// Wire protocol version implemented by this package (`schema_version` 1).
const int protocolVersion = 1;

/// Hard upper bound for one wire frame, matching the Rust codec
/// (`dartq-plugin`, `MAX_FRAME_SIZE`).
const int maxFrameBytes = 16 * 1024 * 1024;

/// Core-to-plugin: the opening handshake request.
const String methodInitialize = 'initialize';

/// Core-to-plugin: the final request; the session ends after its reply.
const String methodShutdown = 'shutdown';

/// Plugin-to-core notification: diagnostics produced by the plugin.
const String methodDiagnosticNotification = 'notification/diagnostic';

/// Core-to-plugin notification: the engine's current diagnostics for the
/// analyzed set, pushed once per analyze cycle as the plugin's diagnostic
/// context (PRD section 14.2). Additively carries the cycle's analyzed file
/// set under `files` — the files a rule-executing plugin runs its enabled
/// rules over.
const String methodEngineDiagnostics = 'notification/engine_diagnostics';

/// Core-to-plugin notification: the plugin's per-cycle execution config —
/// the enabled rule names (resolved by the core from the governing analysis
/// options against the plugin's registration census), the per-code severity
/// overrides, and the open-document overlays (dirty file → unsaved text; the
/// core owns overlays, the host parses them). Pushed before the cycle's
/// `notification/engine_diagnostics` whenever it changed.
const String methodPluginConfig = 'notification/plugin_config';

/// Core-to-plugin: request source fixes for a location.
const String methodRequestFixes = 'request/fixes';

/// Plugin-to-core request (the wire's reverse direction, the T4 Query
/// API): the plugin pulls engine facts — the daemon's index query kinds
/// plus `element_at` — and the core answers with the same method and
/// correlation id. The requesting direction is fixed per method
/// (`request/fixes` only ever core→plugin, `request/query` only ever
/// plugin→core), so one method name disambiguates request from reply on
/// both sides.
const String methodRequestQuery = 'request/query';

/// One wire frame: a request, a reply, or a notification.
///
/// ```json
/// {"schema_version": 1, "id": 7, "method": "initialize", "params": {}}
/// ```
///
/// [id] is null for notifications (plugin-to-core) and an integer for
/// requests (core-to-plugin) and their replies.
class PluginFrame {
  /// A frame with explicit fields; [id] null marks a notification.
  const PluginFrame({
    this.schemaVersion = protocolVersion,
    this.id,
    required this.method,
    this.params = const {},
  });

  /// A request frame (carrying a correlation [id]).
  PluginFrame.request(
    int this.id,
    String this.method, [
    Map<String, Object?> this.params = const {},
  ]) : schemaVersion = protocolVersion;

  /// A notification frame (no correlation id).
  PluginFrame.notification(
    String this.method, [
    Map<String, Object?> this.params = const {},
  ]) : schemaVersion = protocolVersion,
       id = null;

  /// An error reply for the request [id]: `params` carry the failure
  /// message under the `error` key (the wire's only error channel).
  PluginFrame.error(int this.id, String this.method, String message)
    : schemaVersion = protocolVersion,
      params = {'error': message};

  /// Protocol version; must equal [protocolVersion].
  final int schemaVersion;

  /// Request/reply correlation id; null marks a notification.
  final int? id;

  /// The method name (one of the `method*` constants).
  final String method;

  /// Method-specific payload; always a JSON object on the wire.
  final Map<String, Object?> params;

  /// Whether this frame is a notification (no correlation id).
  bool get isNotification => id == null;

  /// The frame as the JSON object written on the wire.
  Map<String, Object?> toJson() => {
    'schema_version': schemaVersion,
    'id': id,
    'method': method,
    'params': params,
  };

  /// Serializes to one JSON line, without the trailing newline.
  String encode() => jsonEncode(toJson());
}

/// A wire-contract violation. Any of these is fatal: the session must be
/// torn down (PRD section 14.3 crash isolation) — there is no
/// resynchronization, matching the Rust codec.
class PluginProtocolException implements Exception {
  /// What broke.
  const PluginProtocolException(this.message);

  /// The failure description.
  final String message;

  @override
  String toString() => 'PluginProtocolException: $message';
}

/// Decodes exactly one frame from one line (trailing newline already
/// stripped).
///
/// Throws [PluginProtocolException] for empty input, malformed JSON, a
/// foreign `schema_version`, or an empty `method` — the typed equivalents of
/// the Rust codec's `ProtocolError`.
PluginFrame decodeFrame(String line) {
  if (line.isEmpty) {
    throw const PluginProtocolException('frame is empty');
  }
  Object? decoded;
  try {
    decoded = jsonDecode(line);
  } on FormatException catch (error) {
    throw PluginProtocolException('frame is not valid JSON: ${error.message}');
  }
  if (decoded is! Map) {
    throw const PluginProtocolException('frame must be a JSON object');
  }
  final map = Map<String, Object?>.from(decoded);

  final schemaVersion = map['schema_version'];
  if (schemaVersion is! int) {
    throw const PluginProtocolException(
      'frame `schema_version` must be an integer',
    );
  }
  if (schemaVersion != protocolVersion) {
    throw PluginProtocolException(
      'unsupported schema_version $schemaVersion; '
      'this build speaks version $protocolVersion',
    );
  }

  final id = map['id'];
  if (id != null && id is! int) {
    throw const PluginProtocolException(
      'frame `id` must be an integer or null',
    );
  }

  final method = map['method'];
  if (method is! String || method.isEmpty) {
    throw const PluginProtocolException(
      'frame `method` must be a non-empty string',
    );
  }

  final params = map['params'] ?? const {};
  if (params is! Map) {
    throw const PluginProtocolException('frame `params` must be a JSON object');
  }

  return PluginFrame(
    schemaVersion: schemaVersion,
    id: id as int?,
    method: method,
    params: Map<String, Object?>.from(params),
  );
}
