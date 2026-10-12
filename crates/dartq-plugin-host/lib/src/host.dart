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

import 'census.dart';
import 'cycle_config.dart';
import 'diagnostic.dart';
import 'edits.dart';
import 'frame.dart';
import 'plugin.dart';
import 'plugin_config.dart';
import 'plugin_session.dart';
import 'query_api.dart';
import 'registry.dart';
import 'rule_runner.dart';

/// Reads newline-delimited frames from [input], drives the session built
/// from [config], and writes each reply line through [send] (one JSON line,
/// without the trailing newline). Plugin-emitted diagnostics go through
/// [onEngineDiagnostics] (its return value is sent back as
/// `notification/diagnostic` frames); the per-cycle execution config goes
/// through [onPluginConfig]; `request/fixes` is served by [onRequestFixes].
/// The session's `initialize` reply advertises [ruleCensus] (the
/// registration census the core resolves enablement against). The same
/// [send] doubles as the reverse-query transport: a plugin's
/// `request/query` pull (T4) must reach the wire while its handler is
/// still awaiting the answer, so the session writes it through this hook
/// instead of the post-frame flush.
///
/// Throws on the first wire-contract violation (malformed UTF-8/JSON,
/// foreign schema version, oversize frame); the process must then exit —
/// there is no resynchronization, matching the Rust codec.
Future<void> runPluginHost(
  PluginConfig config, {
  List<RuleCensus> ruleCensus = const <RuleCensus>[],
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
  EngineDiagnosticsHandler? onEngineDiagnostics,
  PluginConfigHandler? onPluginConfig,
  FixesHandler? onRequestFixes,
}) async {
  final session = PluginSession(
    config: config,
    ruleCensus: ruleCensus,
    onEngineDiagnostics: onEngineDiagnostics,
    onPluginConfig: onPluginConfig,
    onRequestFixes: onRequestFixes,
    writeFrame: (frame) => send(frame.encode()),
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
/// The registered rules execute here (the official server's role): each
/// analyze cycle's config (`notification/plugin_config`) is recorded, and
/// the cycle's batch runs the registry's enabled rules over the analyzed
/// files — real analyzer ASTs the host parses itself, overlays first. The
/// findings ride back as plugin diagnostics, and the `request/fixes` match
/// set is the engine batch together with those own findings: a
/// `registerFixForRule` generator fires on the plugin's own lint finding,
/// not only on a pushed engine row. Every failure path — registration, the
/// wire loop, a producer or a rule that throws — propagates and tears the
/// session down: the core reports the crash (PRD section 14.3; a throwing
/// rule mirrors the official `shouldPropagateExceptions: true`), the reply
/// is never fabricated. A stalled producer is the core's budget to enforce
/// (`callback_ms`); the async loop stays responsive while one computes.
Future<void> servePlugin(
  Plugin plugin, {
  required PluginConfig config,
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
}) async {
  final registry = PluginRegistryImpl();
  await plugin.register(registry);
  await plugin.start();
  // Recording the latest batch and the rules' own findings is the fix
  // context; the batch handler also validates every pushed cycle, and its
  // return value (the rule findings) is emitted back as plugin-owned
  // diagnostics — the engine batch itself is context, never an echo. A
  // plugin that registered no rules runs nothing: the handler still records
  // the batch, at the batch handler's own discretion.
  var engineDiagnostics = const <PluginDiagnostic>[];
  var ownFindings = const <PluginDiagnostic>[];
  var cycleConfig = const PluginCycleConfig();
  try {
    await runPluginHost(
      config,
      ruleCensus: registry.census(),
      input: input,
      send: send,
      onPluginConfig: (config) => cycleConfig = config,
      onEngineDiagnostics: (batch) {
        engineDiagnostics = batch.diagnostics;
        if (registry.lintRules.isEmpty && registry.warningRules.isEmpty) {
          return const <PluginDiagnostic>[];
        }
        return ownFindings = runRules(
          registry: registry,
          config: cycleConfig,
          batch: batch,
        );
      },
      onRequestFixes: (file, offset, length, queries) => _serveFixes(
        registry,
        queries,
        [...engineDiagnostics, ...ownFindings],
        file,
        offset,
        length,
      ),
    );
  } finally {
    await plugin.shutDown();
  }
}

/// Serves one `request/fixes` from the registry: assists at the requested
/// range, rule fixes at their matching diagnostic's range (the engine batch
/// together with the plugin's own rule findings), best first. The
/// producers receive [queries] — the T4 Query API facade — so a
/// resolution-owned fact rides a reverse `request/query` pull.
Future<List<PrioritizedChange>> _serveFixes(
  PluginRegistryImpl registry,
  QueryApi queries,
  List<PluginDiagnostic> fixContext,
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
      ProducerContext(file: file, offset: offset, length: length, queries: queries),
    );
  }
  for (final diagnostic in fixContext) {
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
      queries: queries,
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
