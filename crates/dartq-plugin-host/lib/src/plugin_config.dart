/// The plugin manifest (`dartq_plugin.json`) as a Dart value object.
///
/// Mirrors `Manifest` in the Rust crate `dartq-plugin` (`src/manifest.rs`);
/// both files must change together. Defaults follow PRD section 14.3.
library;

import 'capabilities.dart';

/// Default per-callback wall-clock timeout (PRD section 14.3: 5 s).
const int defaultCallbackTimeoutMs = 5000;

/// Default per-request IPC timeout (PRD section 14.3: 10 s).
const int defaultRequestTimeoutMs = 10000;

/// Default per-request retry budget (PRD section 14.3: 3 retries).
const int defaultRequestRetries = 3;

/// Default memory budget in MB (PRD section 14.3: 512 MB RSS cap).
const int defaultMemoryBudgetMb = 512;

/// Wall-clock and retry budgets of a plugin session. The protocol carries
/// these fields; the core enforces them at runtime (later in M4).
class PluginTimeouts {
  /// Builds explicit budgets.
  const PluginTimeouts({
    this.callbackMs = defaultCallbackTimeoutMs,
    this.requestMs = defaultRequestTimeoutMs,
    this.requestRetries = defaultRequestRetries,
  });

  /// Per-callback wall-clock timeout in milliseconds.
  final int callbackMs;

  /// Per-request IPC timeout in milliseconds.
  final int requestMs;

  /// Retry budget per request.
  final int requestRetries;

  @override
  bool operator ==(Object other) =>
      other is PluginTimeouts &&
      other.callbackMs == callbackMs &&
      other.requestMs == requestMs &&
      other.requestRetries == requestRetries;

  @override
  int get hashCode => Object.hash(callbackMs, requestMs, requestRetries);
}

/// A manifest parse failure; messages always name the offending field.
class PluginConfigException implements Exception {
  /// What is wrong with the manifest.
  const PluginConfigException(this.message);

  /// The failure description.
  final String message;

  @override
  String toString() => 'PluginConfigException: $message';
}

/// A plugin's self-description: identity, entry point, capabilities, and
/// resource budget.
class PluginConfig {
  /// Builds a config; omitted budgets default to the PRD section 14.3
  /// values.
  PluginConfig({
    required this.name,
    required this.version,
    required this.entrypoint,
    CapabilitySet? capabilities,
    this.timeouts = const PluginTimeouts(),
    this.memoryBudgetMb = defaultMemoryBudgetMb,
  }) : capabilities = capabilities ?? CapabilitySet.empty();

  /// Unique plugin name (namespaces its contributions).
  final String name;

  /// Plugin version string.
  final String version;

  /// Entry point the core launches (a Dart script path relative to the
  /// plugin directory).
  final String entrypoint;

  /// Capabilities the plugin advertises; unknown wire names are dropped.
  final CapabilitySet capabilities;

  /// Resource budgets.
  final PluginTimeouts timeouts;

  /// Memory budget in MB.
  final int memoryBudgetMb;

  /// Parses the same JSON document the Rust `Manifest::parse` accepts:
  /// required non-empty strings `name`, `version`, `entrypoint`; optional
  /// `capabilities` (array of wire names, unknown ignored), `timeouts`
  /// (object), and `memory_budget_mb` (positive integer).
  ///
  /// Throws [PluginConfigException] naming the offending field.
  static PluginConfig fromJson(Map<String, Object?> manifest) {
    final name = _requiredString(manifest, 'name');
    final version = _requiredString(manifest, 'version');
    final entrypoint = _requiredString(manifest, 'entrypoint');
    final capabilities = CapabilitySet.parse(manifest['capabilities']);
    final timeouts = _parseTimeouts(manifest['timeouts']);

    final memoryBudgetMb = manifest['memory_budget_mb'];
    if (memoryBudgetMb != null && (memoryBudgetMb is! int || memoryBudgetMb <= 0)) {
      throw const PluginConfigException(
        'manifest field `memory_budget_mb` must be a positive integer',
      );
    }

    return PluginConfig(
      name: name,
      version: version,
      entrypoint: entrypoint,
      capabilities: capabilities,
      timeouts: timeouts,
      memoryBudgetMb:
          memoryBudgetMb is int ? memoryBudgetMb : defaultMemoryBudgetMb,
    );
  }

  static String _requiredString(Map<String, Object?> manifest, String field) {
    final value = manifest[field];
    if (value == null) {
      throw PluginConfigException('manifest field `$field` is required');
    }
    if (value is String && value.isNotEmpty) {
      return value;
    }
    throw PluginConfigException(
      'manifest field `$field` must be a non-empty string',
    );
  }

  static PluginTimeouts _parseTimeouts(Object? value) {
    if (value == null) {
      return const PluginTimeouts();
    }
    if (value is! Map) {
      throw const PluginConfigException(
        'manifest field `timeouts` must be a JSON object',
      );
    }
    final timeouts = Map<String, Object?>.from(value);
    int field(String name, int fallback) {
      final entry = timeouts[name];
      if (entry == null) {
        return fallback;
      }
      if (entry is! int || entry < 0) {
        throw PluginConfigException(
          'manifest field `timeouts.$name` must be a non-negative integer',
        );
      }
      return entry;
    }

    return PluginTimeouts(
      callbackMs: field('callback_ms', defaultCallbackTimeoutMs),
      requestMs: field('request_ms', defaultRequestTimeoutMs),
      requestRetries: field('request_retries', defaultRequestRetries),
    );
  }
}
