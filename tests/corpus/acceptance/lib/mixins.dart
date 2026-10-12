// Valid Dart 3.15 acceptance corpus — mixins (mixin declarations with on/super members, base mixin, mixin application classes (`class C = A with M;`), multi-mixin chains, is-checks on mixins).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/mixin_declaration/, tests/language/mixin/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
mixin Walker {
  int get legs => 2;

  void walk() {
    print('walking on $legs legs');
  }
}

mixin Swimmer on Walker {
  void swim() {
    walk();
    print('swimming');
  }
}

mixin Flyer {
  void fly() {}

  void glide() {
    fly();
  }
}

base mixin Bouncer {
  void bounce() {}
}

class Animal {}

class Duck extends Animal with Walker, Swimmer, Flyer {
  @override
  void fly() {}
}

base class Human = Animal with Walker, Bouncer;

class Bird = Animal with Walker, Flyer;

void main() {
  final duck = Duck();
  duck.walk();
  duck.swim();
  duck.fly();
  final human = Human();
  human.walk();
  human.bounce();
  final bird = Bird();
  bird.glide();
  final zoo = <Object>[duck, human, bird];
  if (zoo[0] is Swimmer && zoo[1] is Walker && zoo[2] is Flyer) {
    print('ok');
  }
}
