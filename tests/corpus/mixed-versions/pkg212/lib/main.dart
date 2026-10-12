// Positive control at language version 2.12 (package default from
// pubspec `sdk: ^2.12.0`, mirrored in .dart_tool/package_config.json —
// anchor: upstream pub source, lib/src/package_config.dart:236-238).
//
// 2.12 is the first null-safe language version: `?` types, `late`, and
// `required` are all legal here. Feature set is derived from the library
// version (anchor: upstream Dart SDK source, pkg/front_end/lib/src/source/
// source_compilation_unit.dart:348-353 — LibraryFeatures from
// languageVersion.version).
//
// Expectation: analyzes clean at 2.12.

int? maybe;

late final String label;

void greet({required String name, int? times}) {
  var text = 'hello $name';
  if (times != null) {
    text = '$text x$times';
  }
  print(text);
}

class Box {
  int value;
  Box(this.value);
}

void main() {
  label = 'ready';
  greet(name: 'dart');
  maybe = Box(1).value;
}
