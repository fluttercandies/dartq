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
/// surfacing. With `query` in the launch args the plugin registers one more
/// producer and negotiates the `query` capability: that producer resolves
/// the requested position through the engine first (`elementAt`, then
/// `references`) and shapes its change from the answers — the T4 Query API
/// face, the "queries let a plugin write smarter fixes" demo.
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

  /// Priority of the query-driven review (the T4 face's producer, registered
  /// only when the `query` launch argument armed it).
  static const int queryReviewPriority = 10;

  /// Prefix of every query-driven change's message (the suffix states the
  /// resolution facts the producer pulled from the engine).
  static const String queryMessagePrefix = 'compat query';

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
    // The T4 face, armed by the `query` launch argument (see [run]): the
    // generator round trips the engine's query face before returning the
    // producer, so the reply's message can state the resolution facts the
    // host could not compute parsed-level.
    if (_queriesEnabled) {
      registry.registerAssist(_queryDrivenReview);
    }
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

/// The query-driven producer (the T4 face): resolves the requested position
/// through the engine before shaping the change. The generator is where the
/// round trips happen ([ProducerGenerator] may be async and is awaited
/// before the reply's message is read): `elementAt` resolves what the
/// position denotes, then `references` tells how live the declaration is —
/// and the returned producer's message and edits already encode those
/// resolution facts, facts a parsed-level producer could not compute. Every
/// failure path degrades to a message-only change: a miss (empty elements)
/// is a legal answer, and a declined or failed query (an unknown file on
/// the error channel, a query-less core) surfaces as a typed [QueryError]
/// the producer catches — the session stays up either way.
Future<Producer> _queryDrivenReview(ProducerContext context) async {
  _crashIfRequested(_crashOnFirstInteraction);
  final queries = context.queries;
  if (queries == null) {
    // No query transport on this host: the documented parsed-level
    // limitation, degraded at the call site.
    return _QueryAnswer(
      CompatPlugin.queryReviewPriority,
      '${CompatPlugin.queryMessagePrefix}: no query transport on this host',
    );
  }
  try {
    // Step 1 — what does the requested position denote? An empty answer is
    // a legal one (nothing there), not a failure.
    final elements = await queries.elementAt(context.file, context.offset);
    if (elements.isEmpty) {
      return _QueryAnswer(
        CompatPlugin.queryReviewPriority,
        '${CompatPlugin.queryMessagePrefix}: nothing resolved at '
        '${context.file}:${context.offset}',
      );
    }
    final element = elements.first;
    // Step 2 — how live is it? (The resolution-owned fact: the reference
    // rows of the symbol face; `queries.symbol` is the combined form.)
    final references = await queries.references(element.name);
    final fileCount = references.map((row) => row.file).toSet().length;
    // Step 3 — the change's shape follows the answer. Unreferenced: the
    // removal-candidate decision, message-only (a safe removal needs more
    // than the name span). Referenced: a concrete edit at the requested
    // range embedding the resolution facts.
    if (references.isEmpty) {
      return _QueryAnswer(
        CompatPlugin.queryReviewPriority,
        '${CompatPlugin.queryMessagePrefix}: `${element.name}` has no '
        'resolved references; a removal candidate',
      );
    }
    return _QueryAnswer(
      CompatPlugin.queryReviewPriority,
      '${CompatPlugin.queryMessagePrefix}: `${element.name}` has '
      '${references.length} reference(s) across $fileCount file(s)',
      [
        SourceEdit(
          offset: context.offset,
          length: context.length,
          replacement:
              '${element.name} /* compat: ${references.length} refs */',
        ),
      ],
    );
  } on QueryError catch (error) {
    // The error channel (an unknown file) or a declined face: degrade to a
    // message-only change — a failed query is not a contract break.
    return _QueryAnswer(
      CompatPlugin.queryReviewPriority,
      '${CompatPlugin.queryMessagePrefix} failed: ${error.message}',
    );
  }
}

/// The resolved query-driven change: the generator did the round trips, so
/// the message and edits are final — [compute] hands them back unchanged.
final class _QueryAnswer implements Producer {
  /// Builds the change; [edits] empty for the message-only faces.
  const _QueryAnswer(this.priority, this.message, [this.edits = const []]);

  @override
  final int priority;

  @override
  final String message;

  @override
  final List<SourceEdit> edits;

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async => edits;
}

/// Whether the intentional-crash mode is armed (set from the launch args by
/// [run]).
bool _crashOnFirstInteraction = false;

/// Whether the T4 query face is armed (set from the launch args by [run]):
/// registers the query-driven producer and negotiates the `query`
/// capability.
bool _queriesEnabled = false;

/// The intentional-crash mode: throws before any reply is produced.
void _crashIfRequested(bool requested) {
  if (requested) {
    throw StateError('compat: intentional crash (sample crash mode)');
  }
}

/// Arms the launch-arg modes from [args] — `crash` (the intentional-crash
/// mode) and `query` (the T4 query face) — and serves the plugin over one
/// stdio session: registration, the host loop (rules included), and the
/// teardown the official lifecycle implies. Used by `bin/compat_plugin.dart`
/// (the launchable entrypoint) and by the test suite (in-memory streams).
Future<void> run(
  List<String> args, {
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
}) {
  _crashOnFirstInteraction = args.contains('crash');
  _queriesEnabled = args.contains('query');
  return servePlugin(
    plugin,
    config: PluginConfig(
      name: plugin.name,
      version: '0.1.0',
      entrypoint: 'bin/compat_plugin.dart',
      capabilities: CapabilitySet([
        Capability.lint,
        Capability.fix,
        if (_queriesEnabled) Capability.query,
      ]),
    ),
    input: input,
    send: send,
  );
}
