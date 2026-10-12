/// Live-compat overlay for the real `riverpod_lint` plugin
/// (`rrousselGit/riverpod` @1efba1c8, pinned under
/// `.third/aswp-samples/riverpod/packages/riverpod_lint`).
///
/// The upstream package is official `analysis_server_plugin`-shaped: a
/// top-level `plugin` object whose `register(PluginRegistry)` runs 16
/// `registerWarningRule` + 6 `registerFixForRule` + 12 `registerAssist`
/// calls — the bulk-registration row of the compat matrix. The overlay
/// imports that object verbatim and runs its real `register` against
/// [RegistryBridge], which implements the official registration face and
/// forwards the two kinds the dartq host mirrors — `registerFixForRule`
/// (keyed by the code's stable string) and `registerAssist` — onto a host
/// `PluginRegistryImpl`.
///
/// T5 closes the registration gap the v1 wire left: `registerLintRule` and
/// `registerWarningRule` forward the real upstream rules onto the host
/// registry, whose rule runner executes them over the cycle's analyzed files
/// at the parsed level (enablement resolved by the core from the census this
/// forwarding feeds). The wire faces stay deterministic bridges for the
/// findings and fixes census: the rules' own findings ride the rule runner,
/// and the pushed-batch census row below remains the registration proof.
library;

import 'package:analysis_server_plugin/registry.dart' as aswp;
import 'package:analyzer/analysis_rule/analysis_rule.dart' show AbstractAnalysisRule;
import 'package:analyzer/error/error.dart' show DiagnosticCode;
import 'package:dartq_plugin_host/dartq_plugin_host.dart' as dartq;
import 'package:riverpod_lint/main.dart' as riverpod_lint;

/// The upstream plugin object, imported verbatim from the pinned clone
/// (typed by its private concrete class; used through the official
/// `Plugin` members it inherits).
final plugin = riverpod_lint.plugin;

/// The manifest's advertised capabilities: riverpod exercises all three.
final config = dartq.PluginConfig(
  name: 'riverpod',
  version: '3.1.9',
  entrypoint: 'bin/riverpod_plugin.dart',
  capabilities: dartq.CapabilitySet([
    dartq.Capability.lint,
    dartq.Capability.fix,
    dartq.Capability.assist,
  ]),
);

/// The marker the fix edits write over the requested range.
const marker = '/* dartq:riverpod */';

/// The upstream plugin's registration, executed for real: the bridge census
/// counts what registered, and the host registry holds the fix/assist
/// delegations the fixes face serves.
Future<RegistryBridge> register() async {
  final bridge = RegistryBridge();
  plugin.register(bridge);
  await plugin.start();
  return bridge;
}

/// Runs the real riverpod plugin over one stdio session. Used by
/// `bin/riverpod_plugin.dart` (the launchable entrypoint) and by the
/// in-process test suite.
Future<void> run({
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
}) async {
  final bridge = await register();
  await dartq.runPluginHost(
    config,
    input: input,
    send: send,
    onEngineDiagnostics: (batch) => [
      // One census row per batch: the sixteen warning rules the host has no
      // mirror for, plus the delegated fix/assist counts, expressed through
      // the wire's lint capability.
      if (batch.diagnostics case [final first, ...])
        dartq.PluginDiagnostic(
          file: first.file,
          severity: dartq.Severity.info,
          code: 'riverpod/census',
          message: 'census lint=${bridge.lintRules}'
              ' warning=${bridge.warningRules}'
              ' fix=${bridge.fixes}'
              ' assist=${bridge.assists}',
        ),
    ],
    onRequestFixes: (file, offset, length, queries) => serveFixes(bridge, offset, length),
  );
  // The core closed its stdin: the session is over.
  await plugin.shutDown();
}

/// Serves one `request/fixes` from the host registry the real registrations
/// landed in: one marker-edit change per registered fix generator (fixes
/// first, in registration order), then the registered assists. Protocol
/// v1's fixes handler is synchronous, so the change set mirrors the
/// registered producers' shape directly (the same posture as the compat
/// sample); the registration remains the source of truth.
List<dartq.PrioritizedChange> serveFixes(
  RegistryBridge bridge,
  int offset,
  int length,
) => [
  for (final entry in bridge.host.fixesByRule.entries)
    for (final generator in entry.value)
      dartq.PrioritizedChange(
        priority: 1,
        message: 'riverpod fix ${entry.key}',
        edits: [dartq.SourceEdit(offset: offset, length: length, replacement: marker)],
      ),
  for (var i = 0; i < bridge.host.assists.length; i++)
    dartq.PrioritizedChange(priority: 50, message: 'riverpod assist ${i + 1}'),
];

/// The registration bridge: the official `PluginRegistry` face the upstream
/// `register` call expects, forwarding the host-mirrored kinds onto
/// [RegistryBridge.host] and counting the rule registrations it has no
/// mirror for.
///
/// Signature note: rule parameters are widened to `Object` and generator
/// parameters to `Function` — the bridge never touches rule internals
/// (census only), which keeps it version-agnostic across the analyzer ranges
/// the pinned clones resolve and independent of the official
/// `CorrectionProducer` types the upstream generators produce.
final class RegistryBridge implements aswp.PluginRegistry {
  /// The host registry the fix and assist delegations land in (the source
  /// of truth the fixes face serves).
  final dartq.PluginRegistryImpl host = dartq.PluginRegistryImpl();

  /// Rules registered through `registerLintRule` (forwarded for real).
  int lintRules = 0;

  /// Rules registered through `registerWarningRule` (forwarded for real).
  int warningRules = 0;

  /// Fix generators delegated to the host registry, in registration order.
  int fixes = 0;

  /// Assist generators delegated to the host registry.
  int assists = 0;

  @override
  void registerLintRule(AbstractAnalysisRule rule) {
    lintRules++;
    // T5: the rule registers for real — the host's rule runner executes it
    // over the cycle's analyzed files (parsed level), enablement resolved by
    // the core from the census this forwarding feeds.
    host.registerLintRule(rule);
  }

  @override
  void registerWarningRule(AbstractAnalysisRule rule) {
    warningRules++;
    host.registerWarningRule(rule);
  }

  @override
  void registerFixForRule(DiagnosticCode code, Function generator) {
    fixes++;
    host.registerFixForRule(
      code.uniqueName,
      (context) async => _MarkerProducer('riverpod fix ${code.uniqueName}'),
    );
  }

  @override
  void registerAssist(Function generator) {
    assists++;
    host.registerAssist(
      (context) async => _MarkerProducer('riverpod assist $assists'),
    );
  }

  @override
  Iterable<Never> enabled(Map<String, Object?> ruleConfigs) =>
      const Iterable<Never>.empty();
}

/// The host-face producer the delegations install: replaces the requested
/// UTF-16 range with the marker. Never invoked by protocol v1's synchronous
/// fixes handler (which mirrors the registration directly), but it is the
/// real `Producer` the host registry holds.
final class _MarkerProducer implements dartq.Producer {
  const _MarkerProducer(this.message);

  @override
  final String message;

  @override
  final int priority = 1;

  @override
  Future<List<dartq.SourceEdit>> compute(dartq.ProducerContext context) async => [
    dartq.SourceEdit(offset: context.offset, length: context.length, replacement: marker),
  ];
}
