// Trigger for riverpod_lint `provider_dependencies`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/provider_dependencies.dart): a
// provider reading another provider without declaring it in
// `dependencies`.
final depProvider = Provider<int>((ref) => 0);

final consumerProvider = Provider<int>((ref) {
  return ref.watch(depProvider);
});

class Provider<T> {
  Provider(this.create);

  final T Function(Ref ref) create;
}

class Ref {
  T watch<T>(Provider<T> provider) => provider.create(this);
}
