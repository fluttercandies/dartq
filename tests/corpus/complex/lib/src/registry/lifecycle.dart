/// Service lifecycle: stateful mixins with caches and hooks, mixed into a
/// concrete service; generic mixins carry their own cache types.
library;

/// Lifecycle hooks mixed into services that need attach/detach symmetry.
mixin Lifecycle {
  bool _attached = false;

  bool get isAttached => _attached;

  void attach() {
    _attached = true;
    onAttach();
  }

  void detach() {
    if (!_attached) {
      return;
    }
    _attached = false;
    onDetach();
  }

  void onAttach() {}

  void onDetach() {}
}

/// Generic caching mixin: the cache key/value types travel with the mixin.
mixin CacheAware<K, V> {
  final Map<K, V> _cache = {};

  V? cached(K key) => _cache[key];

  void remember(K key, V value) {
    _cache[key] = value;
  }

  void forget(K key) {
    _cache.remove(key);
  }

  void clearCache() {
    _cache.clear();
  }

  int get cacheSize => _cache.length;
}

/// The service: two stateful mixins composed, with overrides of the hooks.
class ReceivingService with Lifecycle, CacheAware<String, int> {
  final String dock;

  ReceivingService({required this.dock});

  int processedBatches = 0;

  @override
  void onAttach() {
    processedBatches = 0;
  }

  @override
  void onDetach() {
    clearCache();
  }

  void recordBatch(String skuPrefix) {
    if (!isAttached) {
      return;
    }
    final current = cached(skuPrefix) ?? 0;
    remember(skuPrefix, current + 1);
    processedBatches += 1;
  }
}

/// Audit mixin bound to [ReceivingService] through an on-clause: it can
/// read the service's public members while staying composable.
mixin ReceivingAudit on ReceivingService {
  String auditSummary() =>
      'dock $dock: $processedBatches batches, cache $cacheSize';
}

final class AuditedReceivingService extends ReceivingService
    with ReceivingAudit {
  AuditedReceivingService({required super.dock});
}
