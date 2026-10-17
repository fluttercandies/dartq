// Trigger for riverpod_lint `riverpod_syntax_error`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/riverpod_syntax_error.dart),
// which surfaces riverpod-generator syntax errors (e.g. a notifier missing
// its `build` method reported by the analyzer utils).
class Counter extends Notifier<int> {}

abstract class Notifier<T> {
  T build();
}
