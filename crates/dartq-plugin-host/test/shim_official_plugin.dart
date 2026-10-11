/// The compatibility fixture plugin: written verbatim in official
/// `analysis_server_plugin` style — the same imports and shapes as the
/// official authoring guides (`.third/dart-sdk`,
/// `pkg/analysis_server_plugin/doc/writing_a_plugin.md` and
/// `writing_fixes.md`) — and compiled against the dartq import shim.
///
/// No dartq import appears anywhere in this file: `package:dartq_plugin_host`
/// enters only through the shim's delegation. The official faces that need
/// the analyzer (an `AnalysisRule`-based rule, AST-touching producers) are
/// out of the shim's expressible surface; the fixture registers exactly the
/// two wire-v1 faces — one fix for a rule code, one assist — and asserts
/// nothing itself. `shim_plugin_test.dart` drives it end to end.
library;

import 'package:analysis_server_plugin/edit/assist/assist.dart';
import 'package:analysis_server_plugin/edit/dart/correction_producer.dart';
import 'package:analysis_server_plugin/edit/dart/dart_fix_kind_priority.dart';
import 'package:analysis_server_plugin/edit/fix/fix.dart';
import 'package:analysis_server_plugin/plugin.dart';
import 'package:analysis_server_plugin/registry.dart';

/// The plugin instance the host integrates (the official convention of a
/// top-level `plugin` variable in `lib/main.dart`).
final plugin = ShimFixturePlugin();

class ShimFixturePlugin extends Plugin {
  /// The rule code the fix is registered under, in the official constructor
  /// shape.
  static const code = DiagnosticCode(
    name: 'shim/fixture_rule',
    problemMessage: 'The fixture rule fired.',
    uniqueName: 'shim/fixture_rule',
  );

  @override
  String get name => 'shim fixture';

  @override
  void register(PluginRegistry registry) {
    registry.registerFixForRule(code, FixtureFix.new);
    registry.registerAssist(FixtureAssist.new);
  }
}

/// The fix, in the official `ResolvedCorrectionProducer` authoring shape
/// reduced to the shim's wire-expressible producer base: a const kind, a
/// `{required super.context}` constructor, applicability, and a compute that
/// replaces the anchored selection.
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
  Future<List<SourceEdit>> compute() async => [
    SourceEdit(
      offset: selectionOffset,
      length: selectionLength,
      replacement: '/* shim fix */',
    ),
  ];
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
  Future<List<SourceEdit>> compute() async => [
    SourceEdit(
      offset: selectionOffset,
      length: selectionLength,
      replacement: '/* shim assist */',
    ),
  ];
}
