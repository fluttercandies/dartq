// Positive control at language version 2.17: enhanced enums (members,
// constructors, generics) shipped in 2.17 and are legal at the package
// default. Feature set derives from the library version (anchor:
// upstream Dart SDK source, pkg/front_end/lib/src/source/source_compilation_unit.dart
// :348-353).
//
// Expectation: analyzes clean at 2.17.

enum Color {
  red(1),
  green(2),
  blue(3);

  final int code;

  const Color(this.code);

  bool get isWarm => this == red;
}

enum Profile<T extends Object> {
  single(42),
  pair(1, 2);

  final List<T> values;

  const Profile.single(T v) : values = [v];

  const Profile.pair(T a, T b) : values = [a, b];
}

void main() {
  print(Color.red.isWarm);
  print(Profile.single.values);
}
