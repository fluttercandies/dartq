/// Unit tests for the plugin manifest parser:
/// `package:dartq_plugin_host/plugin_config.dart`.
///
/// Mirrors the Rust `Manifest::parse` unit tests (`dartq-plugin`,
/// `src/manifest.rs`); the Dart side throws `PluginConfigException` naming
/// the offending field.
library;

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

Map<String, Object?> _fullManifest() => {
  'name': 'my_plugin',
  'version': '0.1.0',
  'entrypoint': 'bin/plugin.dart',
  'capabilities': ['lint', 'fix', 'mystery_future_cap'],
  'timeouts': {'callback_ms': 8000, 'request_ms': 20000, 'request_retries': 5},
  'memory_budget_mb': 256,
};

PluginConfigException _configError(Map<String, Object?> manifest) {
  try {
    PluginConfig.fromJson(manifest);
  } on PluginConfigException catch (error) {
    return error;
  }
  fail('expected PluginConfig.fromJson to throw for: $manifest');
}

void main() {
  group('budget defaults (PRD section 14.3)', () {
    test('pin the shared defaults (must match the Rust manifest)', () {
      expect(defaultCallbackTimeoutMs, 5000);
      expect(defaultRequestTimeoutMs, 10000);
      expect(defaultRequestRetries, 3);
      expect(defaultMemoryBudgetMb, 512);
    });
  });

  group('PluginConfig.fromJson', () {
    test('valid manifest parses with all fields', () {
      final config = PluginConfig.fromJson(_fullManifest());
      expect(config.name, 'my_plugin');
      expect(config.version, '0.1.0');
      expect(config.entrypoint, 'bin/plugin.dart');
      expect(config.capabilities.contains(Capability.lint), isTrue);
      expect(config.capabilities.contains(Capability.fix), isTrue);
      expect(config.capabilities.contains(Capability.assist), isFalse);
      expect(
        config.timeouts,
        const PluginTimeouts(
          callbackMs: 8000,
          requestMs: 20000,
          requestRetries: 5,
        ),
      );
      expect(config.memoryBudgetMb, 256);
    });

    test('minimal manifest falls back to default budgets', () {
      final config = PluginConfig.fromJson({
        'name': 'n',
        'version': '1.0.0',
        'entrypoint': 'bin/p.dart',
      });
      expect(config.timeouts, const PluginTimeouts());
      expect(config.memoryBudgetMb, defaultMemoryBudgetMb);
      expect(config.capabilities.isEmpty, isTrue);
    });

    test('missing required fields are named in the typed error', () {
      for (final field in ['name', 'version', 'entrypoint']) {
        final manifest = _fullManifest()..remove(field);
        expect(
          _configError(manifest).message,
          'manifest field `$field` is required',
          reason: 'field: $field',
        );
      }
    });

    test('empty or non-string required fields name the offending field', () {
      expect(
        _configError({..._fullManifest(), 'name': ''}).message,
        'manifest field `name` must be a non-empty string',
      );
      expect(
        _configError({..._fullManifest(), 'version': 42}).message,
        'manifest field `version` must be a non-empty string',
      );
      expect(
        _configError({..._fullManifest(), 'entrypoint': []}).message,
        'manifest field `entrypoint` must be a non-empty string',
      );
    });

    test('memory_budget_mb must be a positive integer when present', () {
      final config = PluginConfig.fromJson({
        ..._fullManifest(),
        'memory_budget_mb': 1,
      });
      expect(config.memoryBudgetMb, 1);

      expect(
        _configError({..._fullManifest(), 'memory_budget_mb': 0}).message,
        'manifest field `memory_budget_mb` must be a positive integer',
      );
      expect(
        _configError({..._fullManifest(), 'memory_budget_mb': -5}).message,
        'manifest field `memory_budget_mb` must be a positive integer',
      );
      expect(
        _configError({..._fullManifest(), 'memory_budget_mb': '256'}).message,
        'manifest field `memory_budget_mb` must be a positive integer',
      );
    });

    test('timeouts must be a JSON object', () {
      expect(
        _configError({..._fullManifest(), 'timeouts': 'fast'}).message,
        'manifest field `timeouts` must be a JSON object',
      );
    });

    test('timeouts entries must be non-negative integers and missing entries '
        'keep their default', () {
      expect(
        _configError({
          ..._fullManifest(),
          'timeouts': {'callback_ms': -1},
        }).message,
        'manifest field `timeouts.callback_ms` must be a non-negative integer',
      );
      expect(
        _configError({
          ..._fullManifest(),
          'timeouts': {'request_retries': 'many'},
        }).message,
        'manifest field `timeouts.request_retries` must be a non-negative integer',
      );

      final partial = PluginConfig.fromJson({
        ..._fullManifest(),
        'timeouts': {'callback_ms': 100},
      });
      expect(partial.timeouts.callbackMs, 100);
      expect(partial.timeouts.requestMs, defaultRequestTimeoutMs);
      expect(partial.timeouts.requestRetries, defaultRequestRetries);
    });
  });

  group('PluginTimeouts value semantics', () {
    test('equal budgets compare equal and hash identically', () {
      const a = PluginTimeouts(callbackMs: 1, requestMs: 2, requestRetries: 3);
      const b = PluginTimeouts(callbackMs: 1, requestMs: 2, requestRetries: 3);
      const different = PluginTimeouts();
      expect(a, b);
      expect(a.hashCode, b.hashCode);
      expect(a == different, isFalse);
    });

    test('exception toString has the typed prefix', () {
      const error = PluginConfigException('manifest field `name` is required');
      expect(
        error.toString(),
        'PluginConfigException: manifest field `name` is required',
      );
    });
  });
}
