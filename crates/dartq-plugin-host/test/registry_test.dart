/// Unit tests for the registration surface:
/// `package:dartq_plugin_host/registry.dart`.
library;

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

Producer _generator(ProducerContext context) => throw UnimplementedError();

Producer _otherGenerator(ProducerContext context) => throw UnimplementedError();

void main() {
  group('PluginRegistryImpl', () {
    test('is a PluginRegistry', () {
      expect(PluginRegistryImpl(), isA<PluginRegistry>());
    });

    test('assists keep registration order', () {
      final registry = PluginRegistryImpl();
      registry.registerAssist(_generator);
      registry.registerAssist(_otherGenerator);
      expect(registry.assists, [_generator, _otherGenerator]);
    });

    test('fixes group by rule code, each list in registration order', () {
      final registry = PluginRegistryImpl();
      registry.registerFixForRule('my_plugin/avoid_sync_io', _generator);
      registry.registerFixForRule('my_plugin/avoid_print', _otherGenerator);
      registry.registerFixForRule('my_plugin/avoid_sync_io', _otherGenerator);

      expect(registry.fixesByRule['my_plugin/avoid_sync_io'], [
        _generator,
        _otherGenerator,
      ]);
      expect(registry.fixesByRule['my_plugin/avoid_print'], [_otherGenerator]);
    });

    test('an unregistered rule code has no entry', () {
      final registry = PluginRegistryImpl();
      registry.registerFixForRule('my_plugin/known', _generator);
      expect(registry.fixesByRule['my_plugin/unknown'], isNull);
    });
  });
}
