/// The host-side session: the server role of the wire contract.
///
/// Mirrors `Session` in the Rust crate `dartq-plugin` (`src/session.rs`),
/// which is the client (core) role; both sides implement the contract
/// specified in that crate's documentation. Pure logic over frames — no I/O,
/// so the transport lives in `host.dart`.
library;

import 'diagnostic.dart';
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

/// Signature of the hook the host calls when the core pushes its engine
/// diagnostics (`notification/engine_diagnostics`, one batch per analyze
/// cycle). Pure function: whatever it returns is emitted back to the core as
/// `notification/diagnostic` frames, in order.
typedef EngineDiagnosticsHandler = List<PluginDiagnostic> Function(
  List<PluginDiagnostic> engineDiagnostics,
);

/// Drives one plugin session: answers the core's `initialize`,
/// `request/fixes`, and `shutdown`, feeds the engine's diagnostics to the
/// plugin, and frames the plugin's diagnostics for delivery.
class PluginSession {
  /// Builds a session for a plugin described by [config].
  PluginSession({required this.config, this.onEngineDiagnostics});

  /// The plugin this session serves (its capabilities are advertised during
  /// the handshake).
  final PluginConfig config;

  /// Called with each engine diagnostics batch the core pushes. Optional:
  /// plugins that only serve `request/fixes` need no subscription.
  final EngineDiagnosticsHandler? onEngineDiagnostics;

  final List<PluginFrame> _outbound = <PluginFrame>[];

  SessionState _state = SessionState.awaitingInitialize;

  /// The current lifecycle state.
  SessionState get state => _state;

  /// Feeds one inbound core frame. Requests return the reply frame to send;
  /// core notifications (`notification/engine_diagnostics`) dispatch to
  /// [onEngineDiagnostics] and return null.
  PluginFrame? handleFrame(PluginFrame frame) {
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
    if (frame.method == methodEngineDiagnostics) {
      _handleEngineDiagnostics(frame);
      return;
    }
    // Protocol v1 has exactly one core-to-plugin notification; anything
    // else is a contract break — fatal, matching the core's machine.
    throw PluginProtocolException(
      'unknown notification `${frame.method}` in state ${_state.name}',
    );
  }

  void _handleEngineDiagnostics(PluginFrame frame) {
    if (_state != SessionState.ready) {
      throw const PluginProtocolException(
        'notification/engine_diagnostics is only valid after initialize completes',
      );
    }
    final entries = frame.params['diagnostics'];
    if (entries is! List) {
      throw const PluginProtocolException(
        'engine_diagnostics payload requires a `diagnostics` array',
      );
    }
    final emitted = onEngineDiagnostics?.call(
      entries.map(_parseEngineDiagnostic).toList(growable: false),
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
    // Reply with this plugin's own capabilities; the core computes the
    // intersection (the wire contract's "intersection wins").
    _state = SessionState.ready;
    return PluginFrame.request(id, frame.method, {
      'protocol_version': protocolVersion,
      'capabilities': config.capabilities.names(),
    });
  }

  PluginFrame _handleFixes(int id, PluginFrame frame) {
    if (_state != SessionState.ready) {
      return PluginFrame.error(
        id,
        frame.method,
        'request/fixes is only valid after initialize completes',
      );
    }
    // Serving real fixes needs the engine Query API (PRD section 14.2),
    // which lands with the engine-side wiring; until then the honest answer
    // for any location is the empty set — the protocol layer is complete.
    return PluginFrame.request(id, frame.method, {'fixes': const []});
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
