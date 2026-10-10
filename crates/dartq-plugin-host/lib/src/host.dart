/// The host main loop: stdio transport for one plugin session.
///
/// The dartiq core spawns this process per plugin (PRD section 14.2) and
/// speaks the wire contract of the Rust crate `dartq-plugin`. The loop
/// enforces the 16 MiB frame bound on raw bytes before any decode, so host
/// memory stays bounded when the core misbehaves.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data' show BytesBuilder;

import 'frame.dart';
import 'plugin_config.dart';
import 'plugin_session.dart';

/// Reads newline-delimited frames from [input], drives the session built
/// from [config], and writes each reply line through [send] (one JSON line,
/// without the trailing newline). Plugin-emitted diagnostics go through
/// [onEngineDiagnostics] (its return value is sent back as
/// `notification/diagnostic` frames); `request/fixes` is served by
/// [onRequestFixes].
///
/// Throws on the first wire-contract violation (malformed UTF-8/JSON,
/// foreign schema version, oversize frame); the process must then exit —
/// there is no resynchronization, matching the Rust codec.
Future<void> runPluginHost(
  PluginConfig config, {
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
  EngineDiagnosticsHandler? onEngineDiagnostics,
  FixesHandler? onRequestFixes,
}) async {
  final session = PluginSession(
    config: config,
    onEngineDiagnostics: onEngineDiagnostics,
    onRequestFixes: onRequestFixes,
  );
  final buffer = BytesBuilder(copy: false);
  await for (final chunk in input) {
    if (buffer.length + chunk.length > maxFrameBytes) {
      buffer.clear();
      throw PluginProtocolException(
        'frame exceeds the $maxFrameBytes-byte frame limit',
      );
    }
    buffer.add(chunk);
    final bytes = buffer.takeBytes();
    var start = 0;
    while (true) {
      final newline = bytes.indexOf(10, start);
      if (newline < 0) {
        break;
      }
      await _serveLine(bytes.sublist(start, newline), session, send);
      // Flush whatever the plugin emitted while this frame was served
      // (replies and `notification/diagnostic` emissions alike), in order.
      for (final frame in session.drainOutbound()) {
        await send(frame.encode());
      }
      start = newline + 1;
    }
    buffer.add(bytes.sublist(start));
  }
}

Future<void> _serveLine(
  List<int> raw,
  PluginSession session,
  Future<void> Function(String line) send,
) async {
  if (raw.isNotEmpty && raw.last == 13) {
    raw = raw.sublist(0, raw.length - 1); // tolerate CRLF
  }
  if (raw.isEmpty) {
    return;
  }
  // Strict UTF-8: malformed bytes throw, tearing the host down.
  final line = utf8.decode(raw);
  final reply = session.handleFrame(decodeFrame(line));
  if (reply != null) {
    await send(reply.encode());
  }
}
