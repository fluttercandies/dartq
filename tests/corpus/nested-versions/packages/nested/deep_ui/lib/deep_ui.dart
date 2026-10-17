// Deep workspace member (depth 4) at language version 3.7. Member-to-
// member import edge (deep_ui -> core) resolved through the single
// root package_config.
//
// Expectation: no errors at 3.7. `runDemo` lacks a return type and
// draws the inherited root-set always_declare_return_types lint — this
// subtree has NO options override, pinning that the root file applies
// at depth 4 while the shell override does not.

import 'package:core/core.dart';

class Panel {
  final String title;
  final int badge;

  const Panel({required this.title, required this.badge});

  String render(int _, int indent) => '${' ' * indent}$title#$badge';

  @override
  String toString() => 'Panel($title#$badge)';
}

runDemo() => const Panel(title: 'main', badge: 7).render(0, 2);

void main() {
  print(runDemo());
}
