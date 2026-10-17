// Trigger for riverpod_lint `scoped_providers_should_specify_dependencies`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/scoped_providers_should_specify_dependencies.dart):
// a scoped provider overriding `dependencies:` with an empty/absent list.
final scopedProvider = Provider<int>(
  (ref) => 0,
  dependencies: const [],
);

class Provider<T> {
  Provider(this.create, {this.dependencies});

  final T Function(Ref ref) create;
  final List<Provider<Object?>>? dependencies;
}

class Ref {}
