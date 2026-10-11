import 'dart:convert';
import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';

/// The generic hook-less entry point the core launches per plugin (the
/// manifest `entrypoint`): serves the wire contract with an empty registry —
/// the handshake, the empty fix set for every `request/fixes`, no
/// diagnostic relay.
///
/// A plugin package points its manifest `entrypoint` at its own thin script
/// instead: import the plugin (the official convention of a top-level
/// `plugin` variable in `lib/main.dart`) and hand the instance to
/// [servePlugin], which runs registration, producers, and lifecycle on this
/// same host loop (`bin/live_plugin.dart` is the reference wiring).
///
/// Optionally takes the path to a `dartq_plugin.json` manifest; without one
/// a default configuration is used. Reads frames from stdin and writes
/// replies to stdout; the process is never started interactively.
Future<void> main(List<String> args) async {
  await runPluginHost(
    _loadConfig(args),
    input: stdin,
    send: (line) async {
      stdout.writeln(line);
      await stdout.flush();
    },
  );
}

PluginConfig _loadConfig(List<String> args) {
  if (args.isEmpty) {
    return PluginConfig(
      name: 'dartq-plugin-host',
      version: '0.1.0',
      entrypoint: 'bin/dartq_plugin_host.dart',
    );
  }
  final decoded = jsonDecode(File(args.first).readAsStringSync());
  if (decoded is! Map) {
    throw const PluginConfigException('manifest must be a JSON object');
  }
  return PluginConfig.fromJson(Map<String, Object?>.from(decoded));
}
