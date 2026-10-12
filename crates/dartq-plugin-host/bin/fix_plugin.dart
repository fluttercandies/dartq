import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';

/// Test plugin for the discovery + fixes round trip (dartq-plugin's
/// `tests/plugin_discovery.rs`): advertises `lint` + `fix`, echoes the first
/// engine diagnostic of each batch back (same contract as `echo_plugin`),
/// and answers `request/fixes` with one trivial change anchored at the
/// requested range, so the Rust side can assert the full discovery -> spawn
/// -> push -> fixes path over a real child process.
///
/// Reads frames on stdin, writes replies and emissions on stdout; never run
/// interactively — the core launches it (manifest `entrypoint`).
Future<void> main() async {
  await runPluginHost(
    PluginConfig(
      name: 'fixer',
      version: '0.1.0',
      entrypoint: 'bin/fix_plugin.dart',
      capabilities: CapabilitySet([Capability.lint, Capability.fix]),
    ),
    input: stdin,
    send: (line) async {
      stdout.writeln(line);
      await stdout.flush();
    },
    onEngineDiagnostics: (batch) {
      if (batch.diagnostics.isEmpty) {
        return const <PluginDiagnostic>[];
      }
      final first = batch.diagnostics.first;
      return <PluginDiagnostic>[
        PluginDiagnostic(
          file: first.file,
          severity: first.severity,
          code: 'fixer/roundtrip',
          message: first.message,
          offset: first.offset,
          length: first.length,
        ),
      ];
    },
    onRequestFixes: (file, offset, length, queries) => <PrioritizedChange>[
      PrioritizedChange(
        priority: 1,
        message: 'insert const',
        edits: [
          SourceEdit(offset: offset, length: length, replacement: 'const '),
        ],
      ),
    ],
  );
}
