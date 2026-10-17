// Trigger for riverpod_lint `unsupported_provider_value`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/unsupported_provider_value.dart):
// a provider returning a value riverpod cannot expose (e.g. a raw
// non-Future/Stream custom object it cannot watch).
final provider = Provider<MyService>((ref) => MyService());

class MyService {
  void work() {}
}

class Provider<T> {
  Provider(this.create);

  final T Function(Ref ref) create;
}

class Ref {}
