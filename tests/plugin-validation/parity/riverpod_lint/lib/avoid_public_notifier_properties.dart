// Trigger for riverpod_lint `avoid_public_notifier_properties`.
// Shape from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/avoid_public_notifier_properties.dart):
// a Notifier subclass exposing public mutable members.
class SettingsNotifier extends Notifier<int> {
  int exposure = 0;

  int get public => exposure;
}
