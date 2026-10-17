/// Live-compat overlay for the real `pyramid_lint` plugin
/// (`charlescyt/pyramid_lint` @v3.0.0, vendored under
/// `vendor/pyramid_lint`).
///
/// The upstream package is official `analysis_server_plugin`-shaped: a
/// top-level `plugin` object whose `register(PluginRegistry)` runs 37
/// `registerLintRule` + 23 `registerFixForRule` + 3 `registerAssist` calls —
/// the lint-only bulk row of the compat matrix. The overlay imports that
/// object verbatim and runs its real `register` against [RegistryBridge],
/// which implements the official registration face and forwards the two
/// kinds the dartq host mirrors — `registerFixForRule` (keyed by the code's
/// stable string) and `registerAssist` — onto a host `PluginRegistryImpl`.
///
/// Two gaps documented here. First, the adapter gap: `registerLintRule` has
/// no host mirror (protocol v1 has no rule registry — the engine's
/// diagnostics batch is the plugin's context), so the 37
/// rule registrations are census-only. Second, the capability posture: the
/// manifest advertises `[lint]` only, so the core never sends
/// `request/fixes` — the 23 fix and 3 assist delegations are provably
/// registered (the census counts them) yet unserved on this session. The
/// wire faces are deterministic bridges, not the upstream rules' semantics
/// (they need the analyzer AST, which the v1 wire does not carry).
library;

import 'package:analysis_server_plugin/plugin.dart' as aswp;
import 'package:analysis_server_plugin/registry.dart' as aswp;
import 'package:analyzer/error/error.dart' show DiagnosticCode;
import 'package:dartq_plugin_host/dartq_plugin_host.dart' as dartq;
import 'package:pyramid_lint/main.dart' as pyramid_lint;

/// The upstream plugin object, imported verbatim from the pinned clone.
final aswp.Plugin plugin = pyramid_lint.plugin;

/// The manifest's advertised capabilities: pyramid is the lint-only row.
final config = dartq.PluginConfig(
  name: 'pyramid',
  version: '3.0.0',
  entrypoint: 'bin/pyramid_plugin.dart',
  capabilities: dartq.CapabilitySet([dartq.Capability.lint]),
);

/// The upstream plugin's registration, executed for real: the bridge census
/// counts what registered, and the host registry holds the fix/assist
/// delegations (unserved — the session advertises lint only).
Future<RegistryBridge> register() async {
  final bridge = RegistryBridge();
  await plugin.register(bridge);
  await plugin.start();
  return bridge;
}

/// Runs the real pyramid plugin over one stdio session. Used by
/// `bin/pyramid_plugin.dart` (the launchable entrypoint) and by the
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
    // No `onRequestFixes`: the session advertises lint only, so the core
    // never sends `request/fixes`.
    onEngineDiagnostics: (batch) => [
      // One census row per batch: the bulk of the upstream registration face
      // (37 lint rules, 23 fix generators, 3 assists), expressed through the
      // wire's lint capability.
      if (batch.diagnostics case [final first, ...])
        dartq.PluginDiagnostic(
          file: first.file,
          severity: dartq.Severity.info,
          code: 'pyramid/census',
          message:
              'census lint=${bridge.lintRules}'
              ' warning=${bridge.warningRules}'
              ' fix=${bridge.fixes}'
              ' assist=${bridge.assists}',
        ),
    ],
  );
  // The core closed its stdin: the session is over.
  await plugin.shutDown();
}

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
  /// The host registry the fix and assist delegations land in. Unserved on
  /// this session: the manifest advertises lint only.
  final dartq.PluginRegistryImpl host = dartq.PluginRegistryImpl();

  /// Rules registered through the mirrorless `registerLintRule`.
  int lintRules = 0;

  /// Rules registered through the mirrorless `registerWarningRule`.
  int warningRules = 0;

  /// Fix generators delegated to the host registry, in registration order.
  int fixes = 0;

  /// Assist generators delegated to the host registry.
  int assists = 0;

  @override
  void registerLintRule(Object rule) => lintRules++;

  @override
  void registerWarningRule(Object rule) => warningRules++;

  @override
  void registerFixForRule(DiagnosticCode code, Function generator) {
    fixes++;
    host.registerFixForRule(
      code.uniqueName,
      (context) async => _MarkerProducer('pyramid fix ${code.uniqueName}'),
    );
  }

  @override
  void registerAssist(Function generator) {
    assists++;
    host.registerAssist(
      (context) async => _MarkerProducer('pyramid assist $assists'),
    );
  }

  @override
  Iterable<Never> enabled(Map<String, Object?> ruleConfigs) =>
      const Iterable<Never>.empty();
}

/// The host-face producer the delegations install: replaces the requested
/// UTF-16 range with the marker. Unserved on this lint-only session, but it
/// is the real `Producer` the host registry holds.
final class _MarkerProducer implements dartq.Producer {
  const _MarkerProducer(this.message);

  @override
  final String message;

  @override
  final int priority = 1;

  @override
  Future<List<dartq.SourceEdit>> compute(dartq.ProducerContext context) async =>
      [
        dartq.SourceEdit(
          offset: context.offset,
          length: context.length,
          replacement: '/* dartq:pyramid */',
        ),
      ];
}
