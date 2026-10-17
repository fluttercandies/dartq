/// Unit tests for the rule runner: `package:dartq_plugin_host/rule_runner.dart`
/// (`runRules`) — the execution of a plugin's enabled analyzer rules over
/// the analyzed set.
///
/// Pins the contract the core depends on: enablement is plain membership
/// (the core resolved the official kind-defaulting against the census),
/// dispatch is per library — parsed when every enabled rule answers
/// `canUseParsedResult`, resolved through a per-cycle
/// `AnalysisContextCollection` otherwise — overlays beat disk and
/// unreadable files are skipped, severity overrides re-level the rows, and
/// a throwing rule propagates (the official
/// `shouldPropagateExceptions: true` posture — one bad rule tears the
/// session down).
library;

import 'dart:io' as io;

import 'package:analyzer/analysis_rule/analysis_rule.dart';
import 'package:analyzer/analysis_rule/rule_context.dart' show RuleContext;
import 'package:analyzer/analysis_rule/rule_visitor_registry.dart'
    show RuleVisitorRegistry;
import 'package:analyzer/src/analysis_rule/rule_context.dart'
    show RuleContextWithResolvedResults;
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

/// A resolution-dependent rule reporting one finding per boolean literal,
/// but only after proving the element model is reachable: it reads
/// `RuleContextWithResolvedResults.typeProvider` at registration, which a
/// parsed context would throw on — so a reported finding proves the library
/// resolved.
class ResolvingRule extends AnalysisRule {
  static const LintCode code = LintCode(
    'runner_resolved',
    'Boolean literals need a resolved review.',
    uniqueName: 'RUNNER_RESOLVED',
    severity: DiagnosticSeverity.WARNING,
  );

  ResolvingRule()
    : super(
        name: 'runner_resolved_review',
        description: 'Test rule: resolved findings per boolean literal.',
      );

  @override
  DiagnosticCode get diagnosticCode => code;

  @override
  bool get canUseParsedResult => false;

  @override
  void registerNodeProcessors(
    RuleVisitorRegistry registry,
    RuleContext context,
  ) {
    // The element-model read that defines "resolved": `typeProvider` throws
    // `UnsupportedError` on a parsed context, so reaching the visitor
    // registration proves the resolved path ran.
    (context as RuleContextWithResolvedResults).typeProvider;
    registry.addBooleanLiteral(this, _Visitor(this));
  }
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
    test('an enabled parse-only rule runs over the analyzed set', () async {
      final findings = await runRules(
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
      () async {
        final findings = await runRules(
          registry: _registry(lint: [BooleanRule()]),
          config: _config(enabledRules: const {}),
          batch: _batch(),
        );
        expect(findings, isEmpty);
      },
    );

    test(
      'enablement is exact on the wire names (both ends push lower-cased)',
      () async {
        final findings = await runRules(
          registry: _registry(lint: [BooleanRule()]),
          config: _config(enabledRules: const {'runner_boolean_review'}),
          batch: _batch(),
        );
        expect(findings, hasLength(1));
        // The wire contract is lower-cased names on both ends (the census
        // keys and the config parse both fold), so an upper-case probe does
        // not match here — case-insensitivity lives at the config boundary.
        final upper = await runRules(
          registry: _registry(lint: [BooleanRule()]),
          config: _config(enabledRules: const {'RUNNER_BOOLEAN_REVIEW'}),
          batch: _batch(),
        );
        expect(upper, isEmpty);
      },
    );
  });

  group('resolution', () {
    // The resolved path needs a real analysis context over real files:
    // `io.Directory.systemTemp` supplies a tree the collection can root,
    // and the overlay provider feeds the file contents.
    late io.Directory sandbox;
    late String libFile;

    /// Runs [rules] (all enabled) over [fileContent] written at
    /// `lib/main.dart` under the sandbox, returned as wire rows.
    Future<List<PluginDiagnostic>> resolve({
      required List<AnalysisRule> rules,
      required String fileContent,
      Map<String, String>? extraFiles,
    }) async {
      libFile = '${sandbox.path}/lib/main.dart';
      io.File(libFile)
        ..parent.createSync(recursive: true)
        ..writeAsStringSync(fileContent);
      extraFiles?.forEach((path, content) {
        io.File('${sandbox.path}/$path')
          ..parent.createSync(recursive: true)
          ..writeAsStringSync(content);
      });
      final registry = _registry(lint: rules);
      final config = PluginCycleConfig(
        enabledRules: {for (final rule in rules) rule.name.toLowerCase()},
        overlays: [
          OverlayEntry(file: libFile, content: fileContent),
          if (extraFiles != null)
            for (final entry in extraFiles.entries)
              OverlayEntry(
                file: '${sandbox.path}/${entry.key}',
                content: entry.value,
              ),
        ],
      );
      return runRules(
        registry: registry,
        config: config,
        batch: _batch([libFile]),
      );
    }

    setUp(() {
      sandbox = io.Directory.systemTemp.createTempSync('dartq_runner_');
    });

    tearDown(() {
      if (sandbox.existsSync()) {
        sandbox.deleteSync(recursive: true);
      }
    });

    test('a resolution-dependent rule resolves and reports', () async {
      final findings = await resolve(
        rules: [ResolvingRule()],
        fileContent: fixture,
      );
      expect(findings, hasLength(1));
      expect(findings.single.code, 'runner_resolved');
      expect(findings.single.severity, Severity.warning);
      expect(findings.single.file, libFile);
      expect(findings.single.offset, 20);
      expect(findings.single.length, 4);
      expect(
        findings.single.message,
        'Boolean literals need a resolved review.',
      );
    });

    test('a parsed-capable rule rides the resolved context too', () async {
      // Upstream runs every enabled rule over the resolved units once the
      // library resolves — the parse-only rule sees the same AST.
      final findings = await resolve(
        rules: [BooleanRule(), ResolvingRule()],
        fileContent: fixture,
      );
      expect(findings, hasLength(2));
      expect(
        findings.map((f) => f.code).toSet(),
        {'runner_boolean', 'runner_resolved'},
      );
    });

    test('a resolution-dependent rule reads the overlay, not the disk',
        () async {
      // The overlay carries two booleans; the disk carries one — resolution
      // must observe the pushed text, matching the parse path's overlay
      // precedence.
      libFile = '${sandbox.path}/lib/main.dart';
      io.File(libFile)
        ..parent.createSync(recursive: true)
        ..writeAsStringSync('void main() { print(true); }\n');
      final findings = await runRules(
        registry: _registry(lint: [ResolvingRule()]),
        config: PluginCycleConfig(
          enabledRules: const {'runner_resolved_review'},
          overlays: [
            OverlayEntry(
              file: libFile,
              content: 'void main() { print(true, false); }\n',
            ),
          ],
        ),
        batch: _batch([libFile]),
      );
      expect(findings, hasLength(2));
    });
  });

  group('file surface', () {
    test('the overlay text is what parses, not the disk', () async {
      final findings = await runRules(
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

    test('a file with no overlay and no disk text is skipped', () async {
      final findings = await runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(overlays: const []),
        batch: _batch(['lib/missing_on_disk.dart']),
      );
      expect(findings, isEmpty);
    });

    test('the same file pushed twice runs once', () async {
      final findings = await runRules(
        registry: _registry(lint: [BooleanRule()]),
        config: _config(),
        batch: _batch(['lib/a.dart', 'lib/a.dart']),
      );
      expect(findings, hasLength(1));
    });

    test('a non-Dart payload still runs the rule (the runner is parse-based, '
        'not extension-based)', () async {
      // The core's analyzed set is the engine's Dart libraries; the runner
      // does not re-filter. A .txt path would simply parse with syntax
      // errors (throwIfDiagnostics: false) and report whatever survives —
      // pinned here as zero findings for prose.
      final findings = await runRules(
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
    test('an override re-levels the row', () async {
      final findings = await runRules(
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

    test('without an override the declared severity stands', () async {
      final findings = await runRules(
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
    test('a throwing rule propagates out of the runner', () async {
      await expectLater(
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
