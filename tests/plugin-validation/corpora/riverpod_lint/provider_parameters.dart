// Trigger for riverpod_lint `provider_parameters`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/provider_parameters.dart): a
// provider whose value is built from a call-site parameter instead of a
// family/dependency.
final provider = Provider<int>((ref) => compute(param));

final param = 1;

int compute(int v) => v;

class Provider<T> {
  Provider(this.create);

  final T Function(Ref ref) create;
}

class Ref {}
