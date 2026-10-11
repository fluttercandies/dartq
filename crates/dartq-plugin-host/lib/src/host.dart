/// The host main loop: stdio transport for one plugin session.
///
/// The dartq core spawns this process per plugin (PRD section 14.2) and
/// speaks the wire contract of the Rust crate `dartq-plugin`. The loop
/// enforces the 16 MiB frame bound on raw bytes before any decode, so host
/// memory stays bounded when the core misbehaves.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data' show BytesBuilder;

import 'diagnostic.dart';
import 'edits.dart';
import 'frame.dart';
import 'plugin.dart';
import 'plugin_config.dart';
import 'plugin_session.dart';
import 'registry.dart';

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
  // Strict UTF-8: malformed bytes throw the typed protocol error, tearing
  // the host down (the counterpart of the Rust codec's `ProtocolError::Utf8`).
  final String line;
  try {
    line = utf8.decode(raw);
  } on FormatException catch (error) {
    throw PluginProtocolException('frame is not valid UTF-8: ${error.message}');
  }
  final reply = await session.handleFrame(decodeFrame(line));
  if (reply != null) {
    await send(reply.encode());
  }
}

/// Serves one wire session for a whole [plugin]: the integration the
/// launchable entry point of a plugin package plays (the role of the
/// official `PluginServer` — import the plugin and hand it over). Runs the
/// lifecycle in contract order — [Plugin.register] into a fresh registry,
/// [Plugin.start], the host loop over [input]/[send], then
/// [Plugin.shutDown] when the session ends, however it ends.
///
/// `request/fixes` is served from the registry, mirroring the official
/// `edit.getFixes` semantics: every registered assist generator runs at the
/// requested range, and every fix generator whose rule code matches an
/// engine diagnostic of the latest pushed batch (`onEngineDiagnostics`
/// context) on the same file overlapping the requested range runs at that
/// diagnostic's range — the official server matches fixes to diagnostics by
/// line; without file content, the diagnostic's own span is the match. The
/// changes are answered best (lowest priority) first, ties in registration
/// order. The batch is the producers' entire context; the engine Query API
/// (PRD section 14.2) will widen it.
///
/// Producer failures propagate and tear the session down: the core reports
/// the crash (PRD section 14.3), the reply is never fabricated. A stalled
/// producer is the core's budget to enforce (`callback_ms`); the async loop
/// stays responsive while one computes.
Future<void> servePlugin(
  Plugin plugin, {
  required PluginConfig config,
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
}) async {
  final registry = PluginRegistryImpl();
  await plugin.register(registry);
  await plugin.start();
  // Recording the latest batch is the fix context; it also validates every
  // pushed batch, and nothing is emitted back — a plugin's own findings
  // need the rule-registry surface (PRD section 14.1), which protocol v1's
  // registration carries no hook for.
  var engineDiagnostics = const <PluginDiagnostic>[];
  try {
    await runPluginHost(
      config,
      input: input,
      send: send,
      onEngineDiagnostics: (batch) {
        engineDiagnostics = batch;
        // The handler's return value is emitted back as plugin-owned
        // diagnostics; the engine batch is context, never an echo.
        return const <PluginDiagnostic>[];
      },
      onRequestFixes: (file, offset, length) =>
          _serveFixes(registry, engineDiagnostics, file, offset, length),
    );
  } finally {
    await plugin.shutDown();
  }
}

/// Serves one `request/fixes` from the registry: assists at the requested
/// range, rule fixes at their matching diagnostic's range, best first.
Future<List<PrioritizedChange>> _serveFixes(
  PluginRegistryImpl registry,
  List<PluginDiagnostic> engineDiagnostics,
  String file,
  int offset,
  int length,
) async {
  final changes = <PrioritizedChange>[];
  Future<void> run(ProducerGenerator generator, ProducerContext context) async {
    final producer = await generator(context);
    changes.add(
      PrioritizedChange(
        priority: producer.priority,
        message: producer.message,
        edits: await producer.compute(context),
      ),
    );
  }

  for (final generator in registry.assists) {
    await run(
      generator,
      ProducerContext(file: file, offset: offset, length: length),
    );
  }
  for (final diagnostic in engineDiagnostics) {
    // The requested and the diagnostic's UTF-16 ranges must overlap.
    if (diagnostic.file != file ||
        diagnostic.offset > offset + length ||
        offset > diagnostic.offset + diagnostic.length) {
      continue;
    }
    final generators = registry.fixesByRule[diagnostic.code];
    if (generators == null) {
      continue;
    }
    final context = ProducerContext(
      file: file,
      offset: diagnostic.offset,
      length: diagnostic.length,
    );
    for (final generator in generators) {
      await run(generator, context);
    }
  }
  // Best (lowest priority) first; ties keep generator order — List.sort is
  // not stable, so the aggregation index is the tie-break.
  final ranked =
      <(int, PrioritizedChange)>[
        for (var i = 0; i < changes.length; i++) (i, changes[i]),
      ]..sort((a, b) {
        final byPriority = a.$2.priority.compareTo(b.$2.priority);
        return byPriority != 0 ? byPriority : a.$1.compareTo(b.$1);
      });
  return [for (final (_, change) in ranked) change];
}
