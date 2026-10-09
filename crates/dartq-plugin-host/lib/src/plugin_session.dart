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

/// Drives one plugin session: answers the core's `initialize`,
/// `request/fixes`, and `shutdown`, and frames the plugin's diagnostics.
class PluginSession {
  /// Builds a session for a plugin described by [config].
  PluginSession({required this.config});

  /// The plugin this session serves (its capabilities are advertised during
  /// the handshake).
  final PluginConfig config;

  SessionState _state = SessionState.awaitingInitialize;

  /// The current lifecycle state.
  SessionState get state => _state;

  /// Feeds one inbound core frame; returns the reply frame to send, or null
  /// when nothing should be written (v1 has no core-to-plugin notifications,
  /// so frames without an id are dropped).
  PluginFrame? handleFrame(PluginFrame frame) {
    final id = frame.id;
    if (id == null) {
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

  /// Frames [diagnostic] for the core; the caller sends the returned frame
  /// over the wire.
  PluginFrame diagnosticFrame(PluginDiagnostic diagnostic) =>
      PluginFrame.notification(methodDiagnosticNotification, diagnostic.toJson());

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
}
