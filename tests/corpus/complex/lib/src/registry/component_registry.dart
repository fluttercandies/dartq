/// Component registry: generic type-keyed registration, mixin classes and
/// an on-clause mixin bound to the component hierarchy.
library;

/// Registry component base. Base class: extensible only inside this
/// library; other libraries consume it as a type.
abstract base class Component {
  String get name;

  void acceptVisit(StringBuffer out) {
    out.write(name);
  }
}

/// Mixin class: usable in `with` chains here and extensible, so a future
/// subclass can grow a constructor without a breaking change.
mixin class Registerable {
  bool _registered = false;

  bool get isRegistered => _registered;

  void markRegistered() {
    _registered = true;
  }
}

/// On-clause mixin: only applicable to components, so [name] resolves.
/// A base supertype forces the `base` modifier on the mixin.
base mixin AuditedComponent on Component {
  int get auditSequence;

  String auditLine() => 'audit #$auditSequence for ${name}';
}

final class StockComponent extends Component with Registerable {
  @override
  final String name;

  final int skusTracked;

  StockComponent({required this.name, required this.skusTracked});

  @override
  String toString() => 'Stock($name, $skusTracked)';
}

final class ColdComponent extends Component with Registerable, AuditedComponent {
  @override
  final String name;

  @override
  final int auditSequence;

  ColdComponent({required this.name, required this.auditSequence});
}

/// Type-keyed registry with generic resolve methods; lookups never cast at
/// call sites because the generic bound reifies the request type.
class ComponentRegistry {
  final Map<Type, List<Object>> _byType = {};

  void register<T extends Object>(T instance) {
    _byType.putIfAbsent(T, () => []).add(instance);
  }

  List<T> resolveAll<T extends Object>() {
    final bucket = _byType[T] ?? const [];
    final matches = <T>[];
    for (final candidate in bucket) {
      if (candidate is T) {
        matches.add(candidate);
      }
    }
    return matches;
  }

  T? resolveFirst<T extends Object>() {
    for (final component in resolveAll<T>()) {
      return component;
    }
    return null;
  }

  int get totalComponents =>
      _bucketsFold(0, (sum, bucket) => sum + bucket.length);

  R _bucketsFold<R>(R seed, R Function(R acc, List<Object> bucket) step) {
    var acc = seed;
    for (final bucket in _byType.values) {
      acc = step(acc, bucket);
    }
    return acc;
  }
}

/// Mixin application form: `class X = Base with Mixin;` declares without a
/// body; abstract base is required because [Component] stays unimplemented
/// and the base supertype forbids an unmodified subclass.
abstract base class ManagedComponent = Component with Registerable;
