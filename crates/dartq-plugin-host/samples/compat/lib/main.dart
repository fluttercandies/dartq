/// The compatibility sample plugin: an official `analysis_server_plugin`-
/// shaped plugin running on the dartq plugin host.
///
/// This file mirrors the layout of the official authoring guide
/// (`.third/dart-sdk/pkg/analysis_server_plugin/doc/writing_a_plugin.md`):
/// a `lib/main.dart` entry exposing a top-level `plugin` variable, a
/// `Plugin` subclass registering its extensions, an analysis rule, and a
/// `CorrectionProducer`-shaped fix. Every concept maps onto a
/// `dartq_plugin_host` equivalent; see `README.md` for the full mapping
/// table.
///
/// The analysis surface is deliberately deterministic: without the engine
/// Query API (PRD section 14.2) a plugin's context is the engine diagnostics
/// batch the core pushes each cycle, so the rule reports a companion finding
/// for every engine row on a `.dart` file, and the registered fix replaces
/// exactly the requested UTF-16 range. Both faces pass UTF-16 offsets
/// through untouched — the position exchange contract.
library;

import 'package:dartq_plugin_host/dartq_plugin_host.dart';

/// The plugin instance the host integrates (the official convention of a
/// top-level `plugin` variable in `lib/main.dart`).
final CompatPlugin plugin = CompatPlugin();

/// The plugin: identity, registration, and lifecycle, mirroring
/// `package:analysis_server_plugin/plugin.dart`.
class CompatPlugin extends Plugin {
  /// The rule code this plugin reports and registers its fix under. The
  /// official `registerFixForRule` takes a `DiagnosticCode`; here the rule
  /// identity is its stable string code.
  static const String ruleCode = 'compat/companion';

  /// Priority of the range-replacement fix (lower sorts first).
  static const int fixPriority = 1;

  /// Priority of the message-only suggestion that follows the fix.
  static const int suggestionPriority = 50;

  /// Title of the range-replacement fix.
  static const String fixMessage = 'Replace range with a compat marker';

  /// The replacement text the fix writes over the requested range.
  static const String fixReplacement = '/* compat: fixed */';

  /// Title of the message-only suggestion.
  static const String suggestionMessage = 'Review this location';

  @override
  String get name => 'compat';

  @override
  void register(PluginRegistry registry) {
    // The official API shape: one fix generator per rule code, plus any
    // assists. The producer is computed per request context.
    registry.registerFixForRule(ruleCode, (context) => const CompatRangeFix());
  }
}

/// The analysis rule, shaped like the official `AnalysisRule` (name,
/// description, one diagnostic code, report-per-context). Protocol v1 has no
/// AST or element access — the engine diagnostics batch pushed with
/// `notification/engine_diagnostics` is the rule's context — so the rule
/// reports one companion finding per engine row on a `.dart` file, at the
/// same UTF-16 range.
class CompanionRule {
  /// The rule's stable name.
  static const String name = 'companion_rule';

  /// One-line description surfaced in tooling.
  static const String description =
      'Reports a companion finding for every engine diagnostic on a .dart'
      ' file, demonstrating the diagnostics flow of the plugin protocol.';

  /// The single diagnostic code this rule reports.
  static const String diagnosticCode = CompatPlugin.ruleCode;

  /// Computes the rule's findings for one engine batch, in arrival order.
  /// Files the rule does not analyze (anything but `.dart`) are skipped.
  List<PluginDiagnostic> report(List<PluginDiagnostic> engineDiagnostics) => [
    for (final row in engineDiagnostics)
      if (row.file.endsWith('.dart'))
        PluginDiagnostic(
          file: row.file,
          severity: Severity.warning,
          code: diagnosticCode,
          message: 'companion for `${row.code}`',
          // UTF-16 in, UTF-16 out: the engine row's range is carried
          // through untouched.
          offset: row.offset,
          length: row.length,
        ),
  ];
}

/// The fix producer, shaped like the official `CorrectionProducer`
/// (priority, message, compute over a request context).
class CompatRangeFix implements Producer {
  /// Builds the fix; const so registrations can share one instance.
  const CompatRangeFix();

  @override
  int get priority => CompatPlugin.fixPriority;

  @override
  String get message => CompatPlugin.fixMessage;

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async => [
    // The requested UTF-16 range, replaced verbatim: the edit's offset and
    // length echo the request, proving the UTF-16 position exchange.
    SourceEdit(
      offset: context.offset,
      length: context.length,
      replacement: CompatPlugin.fixReplacement,
    ),
  ];
}

/// Serves one `request/fixes`, best fix first: the registered range fix
/// anchored at the requested UTF-16 range, then a message-only suggestion.
///
/// Protocol v1's fixes handler is synchronous, so the change set mirrors the
/// registered producer's computation directly (the same posture as the
/// `live_plugin` sample); the registration remains the source of truth for
/// what this plugin fixes.
List<PrioritizedChange> serveFixes(String file, int offset, int length) => [
  PrioritizedChange(
    priority: CompatPlugin.fixPriority,
    message: CompatPlugin.fixMessage,
    edits: [
      // The requested UTF-16 range, replaced verbatim.
      SourceEdit(offset: offset, length: length, replacement: CompatPlugin.fixReplacement),
    ],
  ),
  const PrioritizedChange(
    priority: CompatPlugin.suggestionPriority,
    message: CompatPlugin.suggestionMessage,
  ),
];

/// Runs the plugin over one stdio session: registration, host loop, and the
/// teardown the official lifecycle implies. Used by
/// `bin/compat_plugin.dart` (the launchable entrypoint) and by the test
/// suite (in-memory streams).
///
/// With `crash` in [args] the plugin dies on the first post-handshake
/// interaction: the handlers throw before answering, the unhandled error
/// terminates the process, and the core must report `dartq/plugin_crashed`
/// without following the plugin down — the intentional-crash mode for crash
/// surfacing.
Future<void> run(
  List<String> args, {
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
}) async {
  final crashOnFirstInteraction = args.contains('crash');
  final registry = PluginRegistryImpl();
  plugin.register(registry);
  await plugin.start();
  final rule = CompanionRule();
  await runPluginHost(
    PluginConfig(
      name: plugin.name,
      version: '0.1.0',
      entrypoint: 'bin/compat_plugin.dart',
      capabilities: CapabilitySet([Capability.lint, Capability.fix]),
    ),
    input: input,
    send: send,
    onEngineDiagnostics: (engineDiagnostics) {
      _crashIfRequested(crashOnFirstInteraction);
      return rule.report(engineDiagnostics);
    },
    onRequestFixes: (file, offset, length) {
      _crashIfRequested(crashOnFirstInteraction);
      return serveFixes(file, offset, length);
    },
  );
  // The core closed its stdin: the session is over.
  await plugin.shutDown();
}

/// The intentional-crash mode: throws before any reply is produced.
void _crashIfRequested(bool requested) {
  if (requested) {
    throw StateError('compat: intentional crash (sample crash mode)');
  }
}
