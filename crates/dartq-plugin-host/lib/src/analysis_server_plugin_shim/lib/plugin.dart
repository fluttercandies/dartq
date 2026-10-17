/// The official `Plugin` lifecycle, mirrored verbatim, plus the dartq launch
/// face.
///
/// Mirrors `package:analysis_server_plugin/plugin.dart`
/// (upstream Dart SDK source, `lib/plugin.dart`): [Plugin] has the same four members
/// with the same signatures, so a plugin subclass compiles against this shim
/// unchanged.
///
/// [startPlugin] is the shim's one dartq-specific face: the official package
/// has no process entrypoint (the analysis server generates a bootstrap that
/// imports the plugin's `lib/main.dart` and runs the top-level `plugin`
/// variable over an `analyzer_plugin` channel; the older
/// `pkg/analyzer_plugin/lib/starter.dart` `ServerPluginStarter` plays the
/// same role). Under dartq the core spawns the manifest's `entrypoint`
/// script, and that script's whole body is `startPlugin(plugin)` — the
/// generated-bootstrap stand-in, serving the plugin over the dartq wire v1.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart' as dartq;

import 'registry.dart';

/// A user-visible plugin identity and lifecycle, verbatim the official
/// contract: [name], one [register] callback, [start] per analysis context,
/// [shutDown] on teardown.
abstract class Plugin {
  /// A user-visible name for this plugin, used for error-reporting purposes.
  String get name;

  /// Registers analysis rules, quick fixes, and assists.
  FutureOr<void> register(PluginRegistry registry);

  FutureOr<void> shutDown() {}

  /// Initializes any necessary start-up state that is required for this
  /// plugin to run.
  FutureOr<void> start() {}
}

/// Serves [plugin] over the dartq wire v1 for the lifetime of the process.
///
/// Defaults to the launch contract the dartq core implements
/// (`crates/dartq-plugin`, `SpawnSpec::dart`): the process runs inside the
/// plugin's package directory, reads newline-delimited frames on stdin, and
/// writes replies on stdout; the manifest is `dartq_plugin.json` in the
/// working directory. [input], [send], and [config] let tests (or a custom
/// entrypoint) inject the transport and identity instead.
///
/// Any failure — a missing or malformed manifest, a wire-contract violation,
/// a producer that throws — propagates out of this call, terminating the
/// process. The core reports the crash as one `dartq/plugin_crashed`
/// diagnostic and keeps running; nothing here hangs.
Future<void> startPlugin(
  Plugin plugin, {
  Stream<List<int>>? input,
  Future<void> Function(String line)? send,
  dartq.PluginConfig? config,
}) async {
  final resolvedConfig = config ?? dartq.PluginConfig.fromJson(_readManifest());
  await serveShimPlugin(
    plugin,
    config: resolvedConfig,
    input: input ?? stdin,
    send: send ?? _sendStdout,
  );
}

Map<String, Object?> _readManifest() {
  final raw = File('dartq_plugin.json').readAsStringSync();
  final decoded = jsonDecode(raw);
  if (decoded is Map) {
    return Map<String, Object?>.from(decoded);
  }
  throw const FormatException(
    'dartq_plugin.json must contain a JSON object manifest',
  );
}

Future<void> _sendStdout(String line) async {
  stdout.writeln(line);
  await stdout.flush();
}
