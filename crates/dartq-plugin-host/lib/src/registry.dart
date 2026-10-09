/// The registration surface handed to `Plugin.register`.
///
/// Mirrors `package:analysis_server_plugin/registry.dart`
/// (`.third/dart-sdk`, `lib/registry.dart`): two kinds of edit generators.
/// The official `registerFixForRule` takes the analyzer's `DiagnosticCode`;
/// here the rule code is its stable string identity, so this package carries
/// no analyzer dependency.
library;

import 'edits.dart';

/// The registry a plugin registers its extensions with.
abstract class PluginRegistry {
  /// Registers this assist [generator] with the host.
  void registerAssist(ProducerGenerator generator);

  /// Registers this fix [generator] for the given lint [code].
  void registerFixForRule(String code, ProducerGenerator generator);
}

/// In-memory registry; the host reads it when serving `request/fixes`.
class PluginRegistryImpl implements PluginRegistry {
  /// All registered assist generators, in registration order.
  final List<ProducerGenerator> assists = <ProducerGenerator>[];

  /// Fix generators keyed by the lint rule code they fix.
  final Map<String, List<ProducerGenerator>> fixesByRule =
      <String, List<ProducerGenerator>>{};

  @override
  void registerAssist(ProducerGenerator generator) => assists.add(generator);

  @override
  void registerFixForRule(String code, ProducerGenerator generator) {
    (fixesByRule[code] ??= <ProducerGenerator>[]).add(generator);
  }
}
