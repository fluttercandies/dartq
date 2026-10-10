/// Unit tests for the plugin lifecycle surface:
/// `package:dartq_plugin_host/plugin.dart`.
///
/// The Rust live-fixture tests never invoke a producer's compute; this file
/// closes that gap at the pure level: a real plugin registers real producers
/// and one compute runs against a real [ProducerContext].
library;

import 'dart:async';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

/// A real producer: replaces the requested range with a `const` prefix,
/// exactly the shape of an analyzer quick-fix.
class _AddConstProducer implements Producer {
  const _AddConstProducer();

  @override
  int get priority => 1;

  @override
  String get message => 'Add const';

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async => [
        SourceEdit(
          offset: context.offset,
          length: context.length,
          replacement: 'const ',
        ),
      ];
}

/// A real third-party plugin as it would be authored against this package.
class _AddConstPlugin extends Plugin {
  const _AddConstPlugin();

  @override
  String get name => 'add_const';

  @override
  FutureOr<void> register(PluginRegistry registry) {
    registry.registerAssist((context) => const _AddConstProducer());
    registry.registerFixForRule(
      'my_plugin/avoid_non_const',
      (context) => const _AddConstProducer(),
    );
  }
}

void main() {
  group('Plugin registration and producer compute', () {
    test('register populates the registry; compute turns the context into '
        'edits', () async {
      final plugin = const _AddConstPlugin();
      final registry = PluginRegistryImpl();

      await plugin.start(); // default no-op start-up must be awaitable
      await plugin.register(registry);
      await plugin.shutDown(); // default no-op shutdown must be awaitable

      expect(plugin.name, 'add_const');
      expect(registry.assists, hasLength(1));

      // The assist producer receives the requested location and computes
      // the edit for exactly that range.
      const context = ProducerContext(
        file: 'lib/main.dart',
        offset: 8,
        length: 4,
      );
      final producer = await registry.assists.single(context);
      expect(producer.priority, 1);
      expect(producer.message, 'Add const');

      final edits = await producer.compute(context);
      expect(edits, hasLength(1));
      expect(edits.single.offset, 8);
      expect(edits.single.length, 4);
      expect(edits.single.replacement, 'const ');
      expect(edits.single.toJson(), {
        'offset': 8,
        'length': 4,
        'replacement': 'const ',
      });
    });

    test('rule fixes are retrievable by their stable code', () async {
      final registry = PluginRegistryImpl();
      await const _AddConstPlugin().register(registry);

      final generators = registry.fixesByRule['my_plugin/avoid_non_const'];
      expect(generators, hasLength(1));

      const context = ProducerContext(file: 'a.dart', offset: 0, length: 3);
      final producer = await generators!.single(context);
      final edits = await producer.compute(context);
      expect(edits.single.replacement, 'const ');
    });
  });
}
