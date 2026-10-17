// Mixins x extensions x generics interaction faces: extension members
// losing to concrete interface members, mixin-application
// linearization order, the generic on-constraint checked at the
// application, use-site covariance on overriding parameters, and the
// same-library sealed mixin constraint.
//
// Grounding (upstream Dart SDK source,
// pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart):
// mixin_application_not_implemented_interface — "'{0}' can't be mixed
// onto '{1}' because '{1}' doesn't implement '{2}'."; invalid_override
// — "'{1}.{0}' ('{2}') isn't a valid override of '{3}.{0}' ('{4}')".
//
// Expected rows: 3 —
//   mixin_application_not_implemented_interface:
//     `Orphan with Counted` — the application's superclass Object does
//     not implement Counted's on-type Chained
//     `BrokenCache extends IntStore with Cached<Token>` — IntStore
//     implements Cache<String, int>, the on-type demands
//     Cache<String, Token>, and the instantiation mismatch is judged at
//     the application, not the declaration
//   invalid_override:
//     `LetterDrone.deliver` — a String parameter overriding
//     `void Function(Object?)` without `covariant`

// Extension-vs-member shadowing: RangeExt applies only where the
// receiver's interface lacks the member. TrackedCourse inherits a real
// field from the mixin application, so the extension never enters; the
// plain subclass has no seatCount and dispatches to the extension.
class Course {
  final String title;

  Course(this.title);
}

extension RangeExt on Course {
  int get seatCount => 18;
}

mixin SeatTracking {
  int seatCount = 0;

  void bump() {
    seatCount++;
  }
}

class PlainCourse extends Course {
  PlainCourse(super.title);
}

class TrackedCourse extends Course with SeatTracking {
  TrackedCourse(super.title);
}

void seatingPlan() {
  var plain = PlainCourse('Algebra');
  var tracked = TrackedCourse('Choir');
  // Silence face: no interface member, RangeExt supplies the getter.
  print(plain.seatCount);
  tracked.bump();
  // Silence face: the mixin field shadows the extension member.
  print(tracked.seatCount);
}

// Linearization: later mixins win. Counted constrains itself with
// `on Chained`, and its super.chain resolves down the application's
// chain to Chained.chain — the on-clause makes that reference checkable
// at the declaration while the concrete target is fixed per application.
mixin Chained {
  String get chain => 'base';
}

mixin Counted on Chained {
  @override
  String get chain => 'counted(${super.chain})';
}

class Ledger with Chained, Counted {}

// The row: the superclass of the application is Object, which does not
// implement the on-type Chained.
class Orphan with Counted {}

void linearization() {
  print(Ledger().chain);
}

// Generic on-constraint: Cached's on-type is instantiated per
// application, so the mismatch between Cache<String, int> and
// Cache<String, Token> surfaces only at the breaking class.
abstract class Cache<K, V> {
  V? lookup(K key);
}

class Token {
  final String raw;

  Token(this.raw);
}

class TokenStore implements Cache<String, Token> {
  @override
  Token? lookup(String key) => Token(key);
}

class IntStore implements Cache<String, int> {
  @override
  int? lookup(String key) => key.length;
}

mixin Cached<V> on Cache<String, V> {
  V? memo(String key) => lookup(key);
}

class SessionCache extends TokenStore with Cached<Token> {}

// The row: IntStore does not implement Cache<String, Token>.
class BrokenCache extends IntStore with Cached<Token> {}

void cacheFaces() {
  var session = SessionCache();
  print(session.memo('abc')?.raw);
  print(BrokenCache().memo('abc'));
}

// Use-site covariance: the covariant parameter relaxes the override
// check; the same shape without the modifier is an invalid override.
class Drone {
  void deliver(Object? parcel) {}
}

class FruitDrone extends Drone {
  void deliver(covariant String parcel) {}
}

// The row: String is not a subtype of Object? in parameter position.
class LetterDrone extends Drone {
  void deliver(String parcel) {}
}

void dispatchFleet() {
  var fruit = FruitDrone();
  fruit.deliver('apple');
  var letters = LetterDrone();
  letters.deliver('notice');
}

// Same-library sealed constraint: `on Puzzle` is legal inside the
// library that declares the sealed class, and Crossword satisfies the
// constraint through its superclass. Unarbitrated here (needs a second
// library): the cross-library `on Sealed` face reports
// mixin_on_sealed_class.
sealed class Puzzle {
  const Puzzle();
}

mixin Solvable on Puzzle {
  bool get solved => false;
}

class Crossword extends Puzzle with Solvable {
  final String clue;

  Crossword(this.clue);
}

void puzzleFaces() {
  var daily = Crossword('seven across');
  print(daily.solved);
  print(daily.clue);
}
