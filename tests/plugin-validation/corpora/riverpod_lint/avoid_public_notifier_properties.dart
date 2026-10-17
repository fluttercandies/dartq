// Trigger for riverpod_lint `avoid_public_notifier_properties`.
// Shape from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/avoid_public_notifier_properties.dart):
// a Notifier subclass exposing public mutable members.
// Stand-in: a local `Notifier<T>` keeps the file resolution-free.
class SettingsNotifier extends Notifier<int> {
  int exposure = 0;

  int get public => exposure;

  @override
  int build() => 0;
}

abstract class Notifier<T> {
  T get state => throw UnimplementedError();

  T build();
}
