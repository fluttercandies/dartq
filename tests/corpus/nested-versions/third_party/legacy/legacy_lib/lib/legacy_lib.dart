// Vendored legacy_lib at language version 2.19 (from `sdk: ^2.19.0`)
// — the pre-records baseline of the corpus. Plain 2.12-era shapes only:
// no records, no patterns, no sealed/final class modifiers, no
// switch expressions. Consumed by the 3.7 shell member
// (apps/shell/lib/shell.dart) across the shared resolution.
//
// Expectation: analyzes clean at 2.19.

class LegacyGreeter {
  static String greetLegacy(String who) => 'legacy/$who';
}

enum LegacyMode { eager, lazy }

class Store {
  final LegacyMode mode;
  final Map<String, int> items;

  Store({required this.mode, Map<String, int>? items})
      : items = items ?? const {};

  int? find(String key) => items[key];
}

void main() {
  final s = Store(mode: LegacyMode.lazy);
  print('${LegacyGreeter.greetLegacy('store')} ${s.find('x')}');
}
