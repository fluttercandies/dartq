/// Unit tests for the rule runner: `package:dartq_plugin_host/rule_runner.dart`
/// (`runRules`) — the T5 execution of a plugin's enabled analyzer rules over
/// the analyzed set.
///
/// Pins the contract the core depends on: enablement is plain membership
/// (the core resolved the official kind-defaulting against the census),
/// resolution-dependent rules are skipped (`canUseParsedResult`), overlays
/// beat disk and unreadable files are skipped, severity overrides re-level
/// the rows, and a throwing rule propagates (the official
/// `shouldPropagateExceptions: true` posture — one bad rule tears the
/// session down).
library;

import 'package:analyzer/analysis_rule/analysis_rule.dart';
import 'package:analyzer/analysis_rule/rule_context.dart' show RuleContext;
import 'package:analyzer/analysis_rule/rule_visitor_registry.dart'
    show RuleVisitorRegistry;
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/dart/ast/visitor.dart';
import 'package:analyzer/error/error.dart';
import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

/// The analyzed fixture; `true` sits at UTF-16 offset 20, length 4.
const String fixture = 'void main() { print(true); }\n';

/// A parse-only rule reporting one finding per boolean literal.
class BooleanRule extends AnalysisRule {
  static const LintCode code = LintCode(
    'runner_boolean',
    'Boolean literals need a runner review.',
    uniqueName: 'RUNNER_BOOLEAN',
    severity: DiagnosticSeverity.WARNING,
  );

  BooleanRule()
    : super(
        name: 'runner_boolean_review',
        description: 'Test rule: one finding per boolean literal.',
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

/// A rule identical to [BooleanRule] but resolution-dependent: the runner
/// must skip it (dartq executes parsed-only).
class ResolvingRule extends BooleanRule {
  ResolvingRule() : super();

  @override
  bool get canUseParsedResult => false;
}

/// A rule whose node processor throws mid-visit.
class ThrowingRule extends AnalysisRule {
  ThrowingRule()
    : super(
        name: 'runner_throwing_review',
        description: 'Test rule: throws on the first boolean literal.',
      );

  static const LintCode code = LintCode(
    'runner_throwing',
    'Never reported: the visitor throws first.',
    uniqueName: 'RUNNER_THROWING',
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

class _Visitor extends SimpleAstVisitor<void> {
  _Visitor(this.rule);

  final AnalysisRule rule;

  @override
  void visitBooleanLiteral(BooleanLiteral node) {
    if (rule is ThrowingRule) {
      throw StateError('rule exploded mid-visit');
    }
    rule.reportAtNode(node);
    return null;
  }
}

PluginRegistryImpl _registry({List<AnalysisRule> lint = const []}) {
  final registry = PluginRegistryImpl();
  for (final rule in lint) {
    registry.registerLintRule(rule);
  }
  return registry;
}

EngineBatch _batch([List<String> files = const ['lib/a.dart']]) =>
    EngineBatch(diagnostics: const [], files: files);

PluginCycleConfig _config({
  Set<String> enabledRules = const {'runner_boolean_review'},
  Map<String, Severity> severityOverrides = const {},
  List<OverlayEntry> overlays = const [
    OverlayEntry(file: 'lib/a.dart', content: fixture),
  ],
}) => PluginCycleConfig(
  enabledRules: enabledRules,
  severityOverrides: severityOverrides,
  overlays: overlays,
);

void main() {
  group('enablement', () {
    test('an enabled parse-only rule runs over the analyzed set', () {
      final findings = runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(),
        batch: _batch(),
      );
      expect(findings, hasLength(1));
      expect(findings.single.code, 'runner_boolean');
      expect(findings.single.severity, Severity.warning);
      expect(findings.single.file, 'lib/a.dart');
      expect(findings.single.offset, 20);
      expect(findings.single.length, 4);
      expect(findings.single.message, 'Boolean literals need a runner review.');
    });

    test(
      'a rule outside the enabled set does not run (disabled by default)',
      () {
        final findings = runRules(
          registry: _registry(lint: [BooleanRule()]),
          config: _config(enabledRules: const {}),
          batch: _batch(),
        );
        expect(findings, isEmpty);
      },
    );

    test(
      'enablement is exact on the wire names (both ends push lower-cased)',
      () {
        final findings = runRules(
          registry: _registry(lint: [BooleanRule()]),
          config: _config(enabledRules: const {'runner_boolean_review'}),
          batch: _batch(),
        );
        expect(findings, hasLength(1));
        // The wire contract is lower-cased names on both ends (the census
        // keys and the config parse both fold), so an upper-case probe does
        // not match here — case-insensitivity lives at the config boundary.
        final upper = runRules(
          registry: _registry(lint: [BooleanRule()]),
          config: _config(enabledRules: const {'RUNNER_BOOLEAN_REVIEW'}),
          batch: _batch(),
        );
        expect(upper, isEmpty);
      },
    );

    test('a resolution-dependent rule is skipped, not fatal', () {
      final findings = runRules(
        registry: _registry(lint: [ResolvingRule()]),
        config: _config(),
        batch: _batch(),
      );
      expect(findings, isEmpty);
    });
  });

  group('file surface', () {
    test('the overlay text is what parses, not the disk', () {
      final findings = runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(
          overlays: [
            // Two booleans in the unsaved text — the disk never had them.
            const OverlayEntry(
              file: 'lib/a.dart',
              content: 'void main() { print(true, false); }\n',
            ),
          ],
        ),
        batch: _batch(),
      );
      expect(findings, hasLength(2));
    });

    test('a file with no overlay and no disk text is skipped', () {
      final findings = runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(overlays: const []),
        batch: _batch(['lib/missing_on_disk.dart']),
      );
      expect(findings, isEmpty);
    });

    test('the same file pushed twice runs once', () {
      final findings = runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(),
        batch: _batch(['lib/a.dart', 'lib/a.dart']),
      );
      expect(findings, hasLength(1));
    });

    test('a non-Dart payload still runs the rule (the runner is parse-based, '
        'not extension-based)', () {
      // The core's analyzed set is the engine's Dart libraries; the runner
      // does not re-filter. A .txt path would simply parse with syntax
      // errors (throwIfDiagnostics: false) and report whatever survives —
      // pinned here as zero findings for prose.
      final findings = runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(
          overlays: [
            const OverlayEntry(file: 'notes.txt', content: 'plain prose'),
          ],
        ),
        batch: _batch(['notes.txt']),
      );
      expect(findings, isEmpty);
    });
  });

  group('severity', () {
    test('an override re-levels the row', () {
      final findings = runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(
          severityOverrides: const {'runner_boolean': Severity.error},
        ),
        batch: _batch(),
      );
      expect(findings.single.severity, Severity.error);
      // Only the severity moved.
      expect(findings.single.code, 'runner_boolean');
      expect(findings.single.offset, 20);
    });

    test('without an override the declared severity stands', () {
      final findings = runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(
          severityOverrides: const {'other_code': Severity.error},
        ),
        batch: _batch(),
      );
      expect(findings.single.severity, Severity.warning);
    });
  });

  group('failure posture', () {
    test('a throwing rule propagates out of the runner', () {
      expect(
        () => runRules(
          registry: _registry(lint: [ThrowingRule()]),
          config: _config(enabledRules: const {'runner_throwing_review'}),
          batch: _batch(),
        ),
        throwsStateError,
      );
    });
  });
}
