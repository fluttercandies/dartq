import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';

/// Test plugin for the callback-timeout kill path (dartq-plugin's
/// `tests/callback_timeout.rs`): answers the handshake normally, then stalls
/// in `request/fixes` far past any core-side `callback_ms` budget, so the
/// runtime's per-callback deadline fires, kills this
/// process, and consumes the restart budget.
///
/// Reads frames on stdin, writes replies on stdout; never run interactively
/// — the core launches it (manifest `entrypoint`).
Future<void> main() async {
  await runPluginHost(
    PluginConfig(
      name: 'slow',
      version: '0.1.0',
      entrypoint: 'bin/slow_plugin.dart',
      capabilities: CapabilitySet([Capability.lint, Capability.fix]),
    ),
    input: stdin,
    send: (line) async {
      stdout.writeln(line);
      await stdout.flush();
    },
    // Blocking sleep inside the serve loop: the reply cannot make it out
    // before the core's callback budget expires.
    onRequestFixes: (file, offset, length, queries) {
      sleep(const Duration(seconds: 30));
      return const <PrioritizedChange>[];
    },
  );
}
