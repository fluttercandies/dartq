/// Live-compat overlay for the real `signals_lint` plugin
/// (`rodydavis/signals.dart` @v7.1.0, pinned under
/// `.third/aswp-samples/signals/packages/signals_lint`).
///
/// This upstream package is **not** `analysis_server_plugin`-shaped: it is a
/// `custom_lint` plugin exposing `PluginBase createPlugin()` with
/// `getLintRules`/`getAssists` — no `Plugin.register(PluginRegistry)` and no
/// `registerFixForRule` exist. The adapter gap is therefore structural, and
/// this overlay documents it by driving the real upstream surface directly:
/// `createPlugin()` builds the genuine plugin object, `getLintRules` and
/// `getAssists` produce its five real rules and five real assists, and the
/// census reports what registered (`lint=5 warning=0 fix=0 assist=5`).
///
/// The wire faces are deterministic bridges, not the upstream rules'
/// semantics (those need the analyzer AST; parsed-level AST access is
/// host-local and resolution-owned facts ride the T4 Query API over
/// `request/query`): the lint face reports the
/// census, and the fixes face serves one marker edit per registered assist —
/// the wire has no separate assists method (assists reuse `request/fixes`),
/// so the manifest advertises `fix` for that face.
library;

import 'package:custom_lint_builder/custom_lint_builder.dart' show CustomLintConfigs;
import 'package:dartq_plugin_host/dartq_plugin_host.dart' as dartq;
import 'package:signals_lint/signals_lint.dart' as signals_lint;

/// The upstream plugin object, built by its real entry point.
final plugin = signals_lint.createPlugin();

/// The plugin's real lint rules, as the upstream `getLintRules` reports them.
final rules = plugin.getLintRules(CustomLintConfigs.empty);

/// The plugin's real assists, as the upstream `getAssists` reports them.
final assists = plugin.getAssists();

/// The manifest's advertised capabilities: the census proves the lint rules,
/// and the assists are served through the `request/fixes` face (`fix`).
final config = dartq.PluginConfig(
  name: 'signals',
  version: '7.0.0',
  entrypoint: 'bin/signals_plugin.dart',
  capabilities: dartq.CapabilitySet([dartq.Capability.lint, dartq.Capability.fix]),
);

/// Runs the real signals plugin over one stdio session: the upstream plugin
/// object is built, its rules and assists enumerated, and the wire served
/// from that census. Used by `bin/signals_plugin.dart` (the launchable
/// entrypoint) and by the in-process test suite.
Future<void> run({
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
}) async {
  await dartq.runPluginHost(
    config,
    input: input,
    send: send,
    onEngineDiagnostics: (batch) => [
      // One census row per batch: the registration face of a custom_lint
      // plugin, expressed as the wire's lint capability.
      if (batch.diagnostics case [final first, ...])
        dartq.PluginDiagnostic(
          file: first.file,
          severity: dartq.Severity.info,
          code: 'signals/census',
          message:
              'census lint=${rules.length} warning=0 fix=0 assist=${assists.length}',
        ),
    ],
    onRequestFixes: (file, offset, length, queries) => [
      // One change per registered upstream assist, each replacing the
      // requested UTF-16 range with the marker (the edit-application face).
      for (var i = 0; i < assists.length; i++)
        dartq.PrioritizedChange(
          priority: 50,
          message: 'signals assist ${i + 1}',
          edits: [
            dartq.SourceEdit(
              offset: offset,
              length: length,
              replacement: '/* dartq:signals */',
            ),
          ],
        ),
    ],
  );
}
