// Trigger for riverpod_lint `protected_notifier_properties`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/protected_notifier_properties.dart):
// `Notifier.state` read from outside its own class.
void f(Counter notifier) {
  final value = notifier.state;
}

class Counter extends Notifier<int> {
  @override
  int build() => 0;
}

abstract class Notifier<T> {
  T get state => build();

  T build();
}
