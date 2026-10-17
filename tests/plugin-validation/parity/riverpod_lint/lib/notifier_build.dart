// Trigger for riverpod_lint `notifier_build`.
// Shape from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/notifier_build.dart: "Notifiers
// must have a build method."): a Notifier subclass with no `build` override.
class CounterNotifier extends Notifier<int> {
  void increment() => state++;
}
