/// The registration surface handed to `Plugin.register`.
///
/// Mirrors `package:analysis_server_plugin/registry.dart` plus the analyzer
/// rule registry it extends (`PluginRegistry extends RegistryBase`,
/// `package:analyzer/src/lint/registry.dart`): two kinds of edit generators
/// and two kinds of rules. The official `registerFixForRule` takes the
/// analyzer's `DiagnosticCode`; here the rule code is its stable string
/// identity for the edit face, while rules register as the real analyzer
/// `AbstractAnalysisRule` objects (T5: the host executes them over analyzer
/// ASTs it parses itself — the wire never carries an AST).
library;

import 'package:analyzer/analysis_rule/analysis_rule.dart'
    show AbstractAnalysisRule;

import 'census.dart';
import 'edits.dart';

/// The registry a plugin registers its extensions with.
abstract class PluginRegistry {
  /// Registers this assist [generator] with the host.
  void registerAssist(ProducerGenerator generator);

  /// Registers this fix [generator] for the given lint [code].
  void registerFixForRule(String code, ProducerGenerator generator);

  /// Registers this [rule] with the host. Lint rules are disabled by
  /// default and enabled via the analysis options' rule configs (official
  /// `RegistryBase.registerLintRule` semantics).
  void registerLintRule(AbstractAnalysisRule rule);

  /// Registers this [rule] with the host. Warning rules are enabled by
  /// default and disabled via the analysis options' rule configs (official
  /// `RegistryBase.registerWarningRule` semantics).
  void registerWarningRule(AbstractAnalysisRule rule);
}

/// In-memory registry; the host reads it when serving `request/fixes` and
/// when running the registered rules.
class PluginRegistryImpl implements PluginRegistry {
  /// All registered assist generators, in registration order.
  final List<ProducerGenerator> assists = <ProducerGenerator>[];

  /// Fix generators keyed by the lint rule code they fix.
  final Map<String, List<ProducerGenerator>> fixesByRule =
      <String, List<ProducerGenerator>>{};

  /// Lint rules keyed by their lower-cased names (the official registry's
  /// key form; a later registration of the same name replaces the earlier,
  /// official `registerLintRule` semantics).
  final Map<String, AbstractAnalysisRule> lintRules =
      <String, AbstractAnalysisRule>{};

  /// Warning rules keyed by their lower-cased names.
  final Map<String, AbstractAnalysisRule> warningRules =
      <String, AbstractAnalysisRule>{};

  @override
  void registerAssist(ProducerGenerator generator) => assists.add(generator);

  @override
  void registerFixForRule(String code, ProducerGenerator generator) {
    (fixesByRule[code] ??= <ProducerGenerator>[]).add(generator);
  }

  @override
  void registerLintRule(AbstractAnalysisRule rule) =>
      lintRules[rule.name.toLowerCase()] = rule;

  @override
  void registerWarningRule(AbstractAnalysisRule rule) =>
      warningRules[rule.name.toLowerCase()] = rule;

  /// The rules enabled for the cycle, warning rules first then lint rules
  /// (the official `RegistryBase.enabled` return order). [enabledNames]
  /// carries the core's resolution (the census names it enabled against the
  /// analysis options' rule configs — the official kind-defaulting,
  /// warning-unless-disabled and lint-only-when-enabled, happened there, so
  /// membership here is plain enablement). Resolution-dependent rules are
  /// skipped: dartq executes parsed-only (the T4 boundary) — the official
  /// host resolves them, ours degrades the single rule instead of vetoing
  /// the plugin's turn.
  List<AbstractAnalysisRule> enabled(Set<String> enabledNames) => [
    for (final entry in warningRules.entries)
      if (enabledNames.contains(entry.key) && entry.value.canUseParsedResult)
        entry.value,
    for (final entry in lintRules.entries)
      if (enabledNames.contains(entry.key) && entry.value.canUseParsedResult)
        entry.value,
  ];

  /// The registration census (the `initialize` reply's `rules` array), in
  /// the registry's iteration order: warning rules first, then lint rules.
  List<RuleCensus> census() => [
    for (final entry in warningRules.entries)
      RuleCensus(
        name: entry.key,
        kind: RuleKind.warning,
        codes: codesOf(entry.value),
      ),
    for (final entry in lintRules.entries)
      RuleCensus(
        name: entry.key,
        kind: RuleKind.lint,
        codes: codesOf(entry.value),
      ),
  ];

  /// The rule's codes in wire form: the lower-case name (the fix and dedup
  /// identity) and the declared severity. The analyzer's
  /// `DiagnosticSeverity.name` is upper-case (`ERROR`/`WARNING`/`INFO`;
  /// `NONE` is documented as never used for any code) — the wire wants the
  /// lower-case names.
  static List<RuleCodeInfo> codesOf(AbstractAnalysisRule rule) => [
    for (final code in rule.diagnosticCodes)
      RuleCodeInfo(
        // `name.toLowerCase()` instead of `lowerCaseName`: the sample
        // overlays compile this file against _fe_analyzer_shared 91.0.0,
        // which predates that getter; its definition (93+) is exactly
        // `_name.toLowerCase()`, so the emitted identity is unchanged.
        // ignore: deprecated_member_use
        name: code.name.toLowerCase(),
        severity: switch (code.severity.name) {
          'ERROR' => 'error',
          'INFO' => 'info',
          _ => 'warning',
        },
      ),
  ];
}
