// Trigger for riverpod_lint `avoid_build_context_in_providers`.
// Shape from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/avoid_build_context_in_providers.dart):
// a provider body that reads a BuildContext.
final greetingProvider = Provider((ref) {
  final theme = Theme.of(context);
  return theme;
});
