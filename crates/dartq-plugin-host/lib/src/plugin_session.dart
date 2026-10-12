/// The host-side session: the server role of the wire contract.
///
/// Mirrors `Session` in the Rust crate `dartq-plugin` (`src/session.rs`),
/// which is the client (core) role; both sides implement the contract
/// specified in that crate's documentation. Pure logic over frames — no I/O,
/// so the transport lives in `host.dart`.
library;

import 'dart:async';

import 'census.dart';
import 'cycle_config.dart';
import 'diagnostic.dart';
import 'edits.dart';
import 'frame.dart';
import 'plugin_config.dart';

/// Lifecycle of one plugin session on the host side.
enum SessionState {
  /// No `initialize` request has been answered yet.
  awaitingInitialize,

  /// Handshake complete; work requests may be served.
  ready,

  /// `shutdown` was answered; the session is finished.
  terminated,
}

/// One analyze cycle's push, as `notification/engine_diagnostics` carries
/// it: the engine's current diagnostics plus the cycle's analyzed file set
/// (the files a rule-executing plugin runs its enabled rules over).
final class EngineBatch {
  /// Builds a batch.
  const EngineBatch({required this.diagnostics, this.files = const <String>[]});

  /// The engine's current diagnostics, in push order.
  final List<PluginDiagnostic> diagnostics;

  /// The cycle's analyzed files (wire-form paths).
  final List<String> files;

  /// Parses the `notification/engine_diagnostics` params. `files` is
  /// additive (cores that push no rule surface omit it); when present it
  /// must be an array of strings.
  factory EngineBatch.fromJson(Map<String, Object?> params) {
    final entries = params['diagnostics'];
    if (entries is! List) {
      throw const PluginProtocolException(
        'engine_diagnostics payload requires a `diagnostics` array',
      );
    }
    final diagnostics = entries
        .map(_parseEngineDiagnostic)
        .toList(growable: false);
    final rawFiles = params['files'];
    final List<String> files;
    if (rawFiles == null) {
      files = const <String>[];
    } else if (rawFiles is List) {
      files = [
        for (final file in rawFiles)
          if (file is String)
            file
          else
            throw const PluginProtocolException(
              'engine_diagnostics `files` entries must be strings',
            ),
      ];
    } else {
      throw const PluginProtocolException(
        'engine_diagnostics `files` entries must be strings',
      );
    }
    return EngineBatch(diagnostics: diagnostics, files: files);
  }

  static PluginDiagnostic _parseEngineDiagnostic(Object? entry) {
    if (entry is! Map) {
      throw const PluginProtocolException(
        'engine diagnostic must be a JSON object',
      );
    }
    final map = Map<String, Object?>.from(entry);
    final severityName = map['severity'];
    if (severityName is! String) {
      throw const PluginProtocolException(
        'engine diagnostic field `severity` must be a string',
      );
    }
    final matches = Severity.values
        .where((severity) => severity.name == severityName)
        .toList();
    if (matches.length != 1) {
      throw PluginProtocolException(
        'engine diagnostic field `severity` must be one of '
        '${Severity.values.map((s) => s.name).join('/')}'
        ', got `$severityName`',
      );
    }
    return PluginDiagnostic(
      file: _requireString(map, 'file'),
      severity: matches.single,
      code: _requireString(map, 'code'),
      message: _requireString(map, 'message'),
      offset: _nonNegativeInt(map, 'offset'),
      length: _nonNegativeInt(map, 'length'),
    );
  }

  static String _requireString(Map<String, Object?> map, String field) {
    final value = map[field];
    if (value is String) {
      return value;
    }
    throw PluginProtocolException(
      'engine diagnostic field `$field` must be a string',
    );
  }

  static int _nonNegativeInt(Map<String, Object?> map, String field) {
    final value = map[field] ?? 0;
    if (value is int && value >= 0) {
      return value;
    }
    throw PluginProtocolException(
      'engine diagnostic field `$field` must be a non-negative integer',
    );
  }
}

/// Signature of the hook the host calls when the core pushes its analyze
/// cycle (`notification/engine_diagnostics`). Pure function: whatever it
/// returns is emitted back to the core as `notification/diagnostic` frames,
/// in order.
typedef EngineDiagnosticsHandler =
    List<PluginDiagnostic> Function(EngineBatch batch);

/// Signature of the hook the host calls when the core pushes the per-cycle
/// execution config (`notification/plugin_config`): which census rules are
/// enabled, the per-code severity overrides, and the open-document
/// overlays. Fires before the cycle's engine batch the config governs.
typedef PluginConfigHandler = void Function(PluginCycleConfig config);

/// Signature of the hook serving the core's `request/fixes` (only called
/// when the session is ready). May compute synchronously or return a
/// future: the serve loop awaits it, so a producer that needs async work
/// delays only its own reply — the callback budget (`callback_ms`) stays
/// the core's to enforce, and a stalled reply is answered by the runtime's
/// kill-and-restart (PRD section 14.3). The returned changes are serialized
/// into the reply frame, best (lowest priority) first. A handler failure
/// propagates: the session never fabricates a fix.
typedef FixesHandler =
    FutureOr<List<PrioritizedChange>> Function(
      String file,
      int offset,
      int length,
    );

/// Drives one plugin session: answers the core's `initialize`,
/// `request/fixes`, and `shutdown`, feeds the engine's cycles and the
/// per-cycle config to the plugin, and frames the plugin's diagnostics for
/// delivery.
class PluginSession {
  /// Builds a session for a plugin described by [config]. [ruleCensus] is
  /// the registration census reported in the `initialize` reply (empty for
  /// a plugin that registers no rules).
  PluginSession({
    required this.config,
    this.ruleCensus = const <RuleCensus>[],
    this.onEngineDiagnostics,
    this.onPluginConfig,
    this.onRequestFixes,
  });

  /// The plugin this session serves (its capabilities are advertised during
  /// the handshake).
  final PluginConfig config;

  /// The registration census (the `initialize` reply's `rules` array).
  final List<RuleCensus> ruleCensus;

  /// Called with each engine diagnostics batch the core pushes. Optional:
  /// plugins that only serve `request/fixes` need no subscription.
  final EngineDiagnosticsHandler? onEngineDiagnostics;

  /// Called with each per-cycle execution config the core pushes. Optional:
  /// a plugin without rule execution ignores the config.
  final PluginConfigHandler? onPluginConfig;

  /// Called for each `request/fixes` the core sends. Optional: a plugin
  /// without a fixes handler answers with the empty set — the protocol
  /// layer is complete either way; serving real fixes needs the engine
  /// Query API (PRD section 14.2).
  final FixesHandler? onRequestFixes;

  final List<PluginFrame> _outbound = <PluginFrame>[];

  SessionState _state = SessionState.awaitingInitialize;

  /// The current lifecycle state.
  SessionState get state => _state;

  /// Feeds one inbound core frame. Requests return the reply frame to send;
  /// core notifications (`notification/engine_diagnostics`,
  /// `notification/plugin_config`) dispatch to their handlers and complete
  /// with null. The future completes once the frame is fully served — an
  /// async [onRequestFixes] is awaited before the reply exists, so frames
  /// are served strictly in arrival order.
  Future<PluginFrame?> handleFrame(PluginFrame frame) async {
    final id = frame.id;
    if (id == null) {
      _handleCoreNotification(frame);
      return null;
    }
    switch (frame.method) {
      case methodInitialize:
        return _handleInitialize(id, frame);
      case methodRequestFixes:
        return _handleFixes(id, frame);
      case methodShutdown:
        return _handleShutdown(id, frame);
      default:
        return PluginFrame.error(
          id,
          frame.method,
          'unknown method `${frame.method}`',
        );
    }
  }

  /// Queues [diagnostic] for delivery to the core as
  /// `notification/diagnostic`; the host loop flushes the queue after
  /// serving each inbound frame.
  void emitDiagnostic(PluginDiagnostic diagnostic) => _outbound.add(
    PluginFrame.notification(methodDiagnosticNotification, diagnostic.toJson()),
  );

  /// The frames queued since the last drain, in emit order.
  List<PluginFrame> drainOutbound() {
    if (_outbound.isEmpty) {
      return const <PluginFrame>[];
    }
    final drained = List<PluginFrame>.of(_outbound);
    _outbound.clear();
    return drained;
  }

  void _handleCoreNotification(PluginFrame frame) {
    switch (frame.method) {
      case methodEngineDiagnostics:
        _handleEngineDiagnostics(frame);
      case methodPluginConfig:
        _handlePluginConfig(frame);
      default:
        // The wire has exactly these core-to-plugin notifications; anything
        // else is a contract break — fatal, matching the core's machine.
        throw PluginProtocolException(
          'unknown notification `${frame.method}` in state ${_state.name}',
        );
    }
  }

  void _handlePluginConfig(PluginFrame frame) {
    if (_state != SessionState.ready) {
      throw const PluginProtocolException(
        'notification/plugin_config is only valid after initialize completes',
      );
    }
    onPluginConfig?.call(PluginCycleConfig.fromJson(frame.params));
  }

  void _handleEngineDiagnostics(PluginFrame frame) {
    if (_state != SessionState.ready) {
      throw const PluginProtocolException(
        'notification/engine_diagnostics is only valid after initialize completes',
      );
    }
    final emitted = onEngineDiagnostics?.call(
      EngineBatch.fromJson(frame.params),
    );
    if (emitted != null) {
      emitted.forEach(emitDiagnostic);
    }
  }

  PluginFrame _handleInitialize(int id, PluginFrame frame) {
    if (_state != SessionState.awaitingInitialize) {
      return PluginFrame.error(
        id,
        frame.method,
        'initialize is only valid before the session is ready',
      );
    }
    final requested = frame.params['protocol_version'];
    if (requested != protocolVersion) {
      return PluginFrame.error(
        id,
        frame.method,
        'unsupported protocol_version $requested; '
        'this host speaks version $protocolVersion',
      );
    }
    // Reply with this plugin's own capabilities and its registration
    // census; the core computes the capability intersection (the wire
    // contract's "intersection wins") and resolves rule enablement against
    // the census.
    _state = SessionState.ready;
    return PluginFrame.request(id, frame.method, {
      'protocol_version': protocolVersion,
      'capabilities': config.capabilities.names(),
      'rules': [for (final rule in ruleCensus) rule.toJson()],
    });
  }

  Future<PluginFrame> _handleFixes(int id, PluginFrame frame) async {
    if (_state != SessionState.ready) {
      return PluginFrame.error(
        id,
        frame.method,
        'request/fixes is only valid after initialize completes',
      );
    }
    final handler = onRequestFixes;
    if (handler == null) {
      // No fixes handler: the honest answer for any location is the empty
      // set (real fixes need the engine Query API, PRD section 14.2).
      return PluginFrame.request(id, frame.method, {
        'fixes': const <Object?>[],
      });
    }
    final file = frame.params['file'];
    final offset = frame.params['offset'];
    final length = frame.params['length'];
    if (file is! String || offset is! int || length is! int) {
      throw const PluginProtocolException(
        'request/fixes payload requires string `file`, int `offset`, int `length`',
      );
    }
    final changes = await handler(file, offset, length);
    return PluginFrame.request(id, frame.method, {
      'fixes': [for (final change in changes) change.toJson()],
    });
  }

  PluginFrame _handleShutdown(int id, PluginFrame frame) {
    if (_state != SessionState.ready) {
      return PluginFrame.error(
        id,
        frame.method,
        'shutdown is only valid after initialize completes',
      );
    }
    _state = SessionState.terminated;
    return PluginFrame.request(id, frame.method, const {});
  }
}
