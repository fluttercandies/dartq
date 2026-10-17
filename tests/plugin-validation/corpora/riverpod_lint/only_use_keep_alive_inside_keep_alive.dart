// Trigger for riverpod_lint `only_use_keep_alive_inside_keep_alive`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/only_use_keep_alive_inside_keep_alive.dart):
// a `KeepAliveLink` used inside a provider that is not kept alive.
int dependencyProvider(Ref ref) {
  final link = ref.keepAlive();
  return 0;
}

class Ref {
  KeepAliveLink keepAlive() => KeepAliveLink();
}

class KeepAliveLink {
  void close() {}
}
