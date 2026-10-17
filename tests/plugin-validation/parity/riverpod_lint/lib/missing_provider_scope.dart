// Trigger for riverpod_lint `missing_provider_scope`.
// Shape from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/missing_provider_scope.dart):
// the app entry point runs without a ProviderScope ancestor.
void main() {
  runApp(const App());
}

class App {
  const App();
}
