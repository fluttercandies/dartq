/// The compatibility fixture plugin: written verbatim in official
/// `analysis_server_plugin` style — the same imports and shapes as the
/// official authoring guides (upstream Dart SDK source,
/// `pkg/analysis_server_plugin/doc/writing_a_plugin.md` and
/// `writing_fixes.md`) — and compiled against the dartq import shim.
///
/// No dartq import appears anywhere in this file: `package:dartq_plugin_host`
/// enters only through the shim's delegation. The fixture registers the
/// three wire-v1 faces — one real analyzer lint rule (T5: the shim is typed
/// against the analyzer's `AbstractAnalysisRule`, the same dependency the
/// official package carries), fixes for rule codes (one extending the merged
/// base directly, one extending the authoring-family
/// `ResolvedCorrectionProducer`), one assist — and asserts nothing itself.
/// `shim_plugin_test.dart` drives it end to end.
library;

import 'package:analysis_server_plugin/edit/assist/assist.dart';
import 'package:analysis_server_plugin/edit/change_builder/change_builder.dart';
import 'package:analysis_server_plugin/edit/dart/correction_producer.dart';
import 'package:analysis_server_plugin/edit/dart/dart_fix_kind_priority.dart';
import 'package:analysis_server_plugin/edit/fix/fix.dart';
import 'package:analysis_server_plugin/plugin.dart';
import 'package:analysis_server_plugin/registry.dart';
import 'package:analyzer/analysis_rule/analysis_rule.dart';
import 'package:analyzer/analysis_rule/rule_context.dart' show RuleContext;
import 'package:analyzer/analysis_rule/rule_visitor_registry.dart'
    show RuleVisitorRegistry;
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/dart/ast/visitor.dart';
import 'package:analyzer/error/error.dart';
import 'package:analyzer/source/source_range.dart' show SourceRange;

/// The plugin instance the host integrates (the official convention of a
/// top-level `plugin` variable in `lib/main.dart`).
final plugin = ShimFixturePlugin();

class ShimFixturePlugin extends Plugin {
  /// The rule code the fix is registered under, in the official shape (the
  /// concrete `LintCode`, exactly what official plugins construct).
  static const code = LintCode(
    'shim/fixture_rule',
    'The fixture rule fired.',
    uniqueName: 'shim/fixture_rule',
  );

  /// The code the authoring-family fix registers under (a distinct code so
  /// the two families stay separately addressable on the wire).
  static const resolvedCode = LintCode(
    'shim/fixture_resolved_rule',
    'The fixture resolved rule fired.',
    uniqueName: 'shim/fixture_resolved_rule',
  );

  @override
  String get name => 'shim fixture';

  @override
  void register(PluginRegistry registry) {
    registry.registerLintRule(FixtureRule());
    registry.registerFixForRule(code, FixtureFix.new);
    registry.registerFixForRule(resolvedCode, FixtureResolvedFix.new);
    registry.registerAssist(FixtureAssist.new);
  }
}

/// The lint rule, in the official one-code `AnalysisRule` shape: one finding
/// per boolean literal — deterministic and parse-only, so the shim's wire
/// session executes it exactly like the official server would.
class FixtureRule extends AnalysisRule {
  /// The rule's code (the census row and the emitted findings' identity).
  static const LintCode ruleCode = LintCode(
    'fixture_boolean',
    'Boolean literals need a fixture review.',
    uniqueName: 'FIXTURE_BOOLEAN',
  );

  /// Builds the rule (the official `AnalysisRule` constructor).
  FixtureRule()
    : super(
        name: 'fixture_boolean_review',
        description:
            'Reports one finding per boolean literal, exercising the'
            ' shim-registered rule execution.',
      );

  @override
  DiagnosticCode get diagnosticCode => ruleCode;

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

  final FixtureRule rule;

  @override
  void visitBooleanLiteral(BooleanLiteral node) {
    rule.reportAtNode(node);
  }
}

/// The fix, in the official producer shape: a const kind, a
/// `{required super.context}` constructor, applicability, and the official
/// compute writing its replacement into the real `ChangeBuilder` (a generic
/// file edit — the shim's wire workspace carries no analysis session, and a
/// plain range replacement needs none).
class FixtureFix extends CorrectionProducer {
  static const _kind = FixKind(
    'shim.fix.fixtureFix',
    DartFixKindPriority.standard,
    'Fix the fixture range',
  );

  FixtureFix({required super.context});

  @override
  CorrectionApplicability get applicability =>
      CorrectionApplicability.singleLocation;

  @override
  FixKind get fixKind => _kind;

  @override
  Future<void> compute(ChangeBuilder builder) async {
    await builder.addGenericFileEdit(file, (fileBuilder) {
      fileBuilder.addSimpleReplacement(
        SourceRange(selectionOffset, selectionLength),
        '/* shim fix */',
      );
    });
  }
}

/// The assist, same shape as [FixtureFix]. Its priority deliberately exceeds
/// the fix's so the wire reply pins the official ordering: the official
/// `Assist.compareFixes` sorts higher priority first, and the shim's adapter
/// negates onto the wire's lower-first order.
class FixtureAssist extends CorrectionProducer {
  static const _kind = AssistKind(
    'shim.assist.fixtureAssist',
    DartFixKindPriority.standard + 10,
    'Assist the fixture range',
  );

  FixtureAssist({required super.context});

  @override
  CorrectionApplicability get applicability =>
      CorrectionApplicability.singleLocation;

  @override
  AssistKind get assistKind => _kind;

  @override
  Future<void> compute(ChangeBuilder builder) async {
    await builder.addGenericFileEdit(file, (fileBuilder) {
      fileBuilder.addSimpleReplacement(
        SourceRange(selectionOffset, selectionLength),
        '/* shim assist */',
      );
    });
  }
}

/// The authoring-family fix: extends `ResolvedCorrectionProducer`, the
/// official resolved base, where [FixtureFix] extends the merged
/// [CorrectionProducer] face directly. The resolved constructor tearoff
/// still fits the official `ProducerGenerator` shape — `ResolvedUnitResult
/// implements ParsedUnitResult` supplies the covariant step — so
/// registration validates its kind through the same census path. Its
/// resolution-owned members (`unitResult`, `typeProvider`, ...) stay the
/// shim's typed boundary and are never reached: the compute writes a
/// generic file edit, which needs no analysis session.
class FixtureResolvedFix extends ResolvedCorrectionProducer {
  static const _kind = FixKind(
    'shim.fix.fixtureResolvedFix',
    DartFixKindPriority.standard,
    'Fix the fixture range (resolved)',
  );

  FixtureResolvedFix({required super.context});

  @override
  CorrectionApplicability get applicability =>
      CorrectionApplicability.singleLocation;

  @override
  FixKind get fixKind => _kind;

  @override
  Future<void> compute(ChangeBuilder builder) async {
    await builder.addGenericFileEdit(file, (fileBuilder) {
      fileBuilder.addSimpleReplacement(
        SourceRange(selectionOffset, selectionLength),
        '/* shim resolved fix */',
      );
    });
  }
}
