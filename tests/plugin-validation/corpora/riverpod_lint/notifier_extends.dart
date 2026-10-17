// Trigger for riverpod_lint `notifier_extends`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/notifier_extends.dart): a
// notifier class that does not extend the matching generated `_$` base.
class Counter extends Notifier<int> {
  @override
  int build() => 0;
}

abstract class Notifier<T> {
  T build();
}
