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
/// The analysis surface is a real analyzer lint rule (T5): the host executes
/// the registered rule over the cycle's analyzed files, parsing them itself
/// — the rule code is ordinary analyzer-plugin code (`registerLintRule`
/// with an `AbstractAnalysisRule` subclass), disabled by default and enabled
/// by the core's per-cycle config. The registered fix matches the rule's own
/// findings (the fix context is the engine batch together with the rule's
/// emissions); both faces pass UTF-16 offsets — the position exchange
/// contract. With `crash` in the launch args the producer dies on the first
/// post-handshake interaction: the intentional-crash mode for crash
/// surfacing.
library;

import 'package:analyzer/analysis_rule/analysis_rule.dart';
import 'package:analyzer/analysis_rule/rule_context.dart' show RuleContext;
import 'package:analyzer/analysis_rule/rule_visitor_registry.dart'
    show RuleVisitorRegistry;
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/dart/ast/visitor.dart';
import 'package:analyzer/error/error.dart';
import 'package:dartq_plugin_host/dartq_plugin_host.dart';

/// The plugin instance the host integrates (the official convention of a
/// top-level `plugin` variable in `lib/main.dart`).
final CompatPlugin plugin = CompatPlugin();

/// The plugin: identity, registration, and lifecycle, mirroring
/// `package:analysis_server_plugin/plugin.dart`.
class CompatPlugin extends Plugin {
  /// The rule code this plugin's rule reports and its fix registers under
  /// (the official `registerFixForRule` takes a `DiagnosticCode`; the dartq
  /// host keys the fix face by the code's lower-case name).
  static const String ruleCode = 'compat/boolean_literal';

  /// Priority of the range-replacement fix (lower sorts first).
  static const int fixPriority = 1;

  /// Priority of the message-only suggestion (registered as an assist).
  static const int suggestionPriority = 50;

  /// Title of the range-replacement fix.
  static const String fixMessage = 'Replace literal with a compat marker';

  /// The replacement text the fix writes over the finding's range.
  static const String fixReplacement = '/* compat: fixed */';

  /// Title of the message-only suggestion.
  static const String suggestionMessage = 'Review this location';

  @override
  String get name => 'compat';

  @override
  void register(PluginRegistry registry) {
    // The official API shape: one lint rule (disabled by default — the
    // core's config enables it), one fix generator for the rule's code, and
    // one assist. The host keys the fix face by the code's lower-case name
    // (what the official registry stores).
    registry.registerLintRule(BooleanLiteralRule());
    registry.registerFixForRule(
      BooleanLiteralRule.code.lowerCaseName,
      (context) => const CompatRangeFix(),
    );
    registry.registerAssist((context) => const CompatSuggestion());
  }
}

/// The analysis rule: an ordinary analyzer-plugin rule (`AnalysisRule`
/// subclass, exactly the shape the official `registerLintRule` takes). It
/// reports one finding per boolean literal — deterministic, parse-only, and
/// fixture-friendly: `void main() { print(true); }` yields exactly one
/// finding at the literal.
class BooleanLiteralRule extends AnalysisRule {
  /// The rule's code, declared once as a `LintCode` (the concrete
  /// `DiagnosticCode` the official one-code rule shape uses).
  static const LintCode code = LintCode(
    'boolean_literal',
    'Boolean literals need a compat review.',
    uniqueName: 'COMPAT_BOOLEAN_LITERAL',
    severity: DiagnosticSeverity.WARNING,
  );

  /// Builds the rule (the official `AnalysisRule` constructor).
  BooleanLiteralRule()
    : super(
        name: 'boolean_literal_review',
        description:
            'Reports one finding per boolean literal, demonstrating'
            ' the rule-execution flow of the dartq plugin host.',
      );

  @override
  DiagnosticCode get diagnosticCode => code;

  @override
  bool get canUseParsedResult => true;

  @override
  void registerNodeProcessors(
    RuleVisitorRegistry registry,
    RuleContext context,
  ) {
    registry.addBooleanLiteral(this, _Visitor(this));
  }
}

/// The rule's node processor: one report per boolean literal.
class _Visitor extends SimpleAstVisitor<void> {
  _Visitor(this.rule);

  final BooleanLiteralRule rule;

  @override
  void visitBooleanLiteral(BooleanLiteral node) {
    rule.reportAtNode(node);
  }
}

/// The fix producer, shaped like the official `CorrectionProducer` (priority,
/// message, compute over a request context): it replaces the fixing
/// diagnostic's own span — the host hands the matching finding's range to
/// `registerFixForRule` generators, not the requested one.
class CompatRangeFix implements Producer {
  /// Builds the fix; const so registrations can share one instance.
  const CompatRangeFix();

  @override
  int get priority => CompatPlugin.fixPriority;

  @override
  String get message => CompatPlugin.fixMessage;

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async {
    _crashIfRequested(_crashOnFirstInteraction);
    return [
      SourceEdit(
        offset: context.offset,
        length: context.length,
        replacement: CompatPlugin.fixReplacement,
      ),
    ];
  }
}

/// The assist producer: a message-only suggestion at the requested range
/// (no edits) — the second face of the fixes protocol, served through the
/// same `request/fixes` method, sorted after the fix by priority.
class CompatSuggestion implements Producer {
  /// Builds the suggestion; const so registrations can share one instance.
  const CompatSuggestion();

  @override
  int get priority => CompatPlugin.suggestionPriority;

  @override
  String get message => CompatPlugin.suggestionMessage;

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async {
    _crashIfRequested(_crashOnFirstInteraction);
    return const <SourceEdit>[];
  }
}

/// Whether the intentional-crash mode is armed (set from the launch args by
/// [runWith]).
bool _crashOnFirstInteraction = false;

/// The intentional-crash mode: throws before any reply is produced.
void _crashIfRequested(bool requested) {
  if (requested) {
    throw StateError('compat: intentional crash (sample crash mode)');
  }
}

/// Arms the crash mode from [args] and serves the plugin over one stdio
/// session: registration, the host loop (rules included), and the teardown
/// the official lifecycle implies. Used by `bin/compat_plugin.dart` (the
/// launchable entrypoint) and by the test suite (in-memory streams).
Future<void> run(
  List<String> args, {
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
}) {
  _crashOnFirstInteraction = args.contains('crash');
  return servePlugin(
    plugin,
    config: PluginConfig(
      name: plugin.name,
      version: '0.1.0',
      entrypoint: 'bin/compat_plugin.dart',
      capabilities: CapabilitySet([Capability.lint, Capability.fix]),
    ),
    input: input,
    send: send,
  );
}
