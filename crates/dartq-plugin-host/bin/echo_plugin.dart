import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';

/// Test plugin for the engine-wiring round trip (dartq-plugin's
/// `tests/e2e_round_trip.rs`): subscribes to the core's engine diagnostics
/// and echoes the first diagnostic of each batch back as its own finding,
/// re-coded `echo/roundtrip`, so the Rust side can assert the full
/// core -> plugin -> core path over a real child process.
///
/// Reads frames on stdin, writes replies and emissions on stdout; never run
/// interactively — the core launches it (manifest `entrypoint`).
Future<void> main() async {
  await runPluginHost(
    PluginConfig(
      name: 'echo',
      version: '0.1.0',
      entrypoint: 'bin/echo_plugin.dart',
      capabilities: CapabilitySet([Capability.lint]),
    ),
    input: stdin,
    send: (line) async {
      stdout.writeln(line);
      await stdout.flush();
    },
    onEngineDiagnostics: (diagnostics) {
      if (diagnostics.isEmpty) {
        return const <PluginDiagnostic>[];
      }
      final first = diagnostics.first;
      return <PluginDiagnostic>[
        PluginDiagnostic(
          file: first.file,
          severity: first.severity,
          code: 'echo/roundtrip',
          message: first.message,
          offset: first.offset,
          length: first.length,
        ),
      ];
    },
  );
}
