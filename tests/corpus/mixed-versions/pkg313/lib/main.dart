// Everything-valid baseline at the newest pinned ladder version (3.13):
// records, patterns, class modifiers (3.0+), extension types (3.3+), and
// digit separators (3.6+) coexist in one file.
//
// Expectation: analyzes clean at 3.13.

sealed class Animal {}

final class Dog extends Animal {
  final String name;

  Dog(this.name);
}

final class Cat extends Animal {}

extension type Grams(int value) {}

const int grams = 5_000;

String sound(Animal a) {
  return switch (a) {
    Dog(name: var n) => 'woof from $n',
    Cat() => 'meow',
  };
}

void main() {
  print(sound(Dog('rex')));
  print(Grams(grams).value);
}
