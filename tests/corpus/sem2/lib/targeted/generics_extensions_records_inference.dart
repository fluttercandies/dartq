// Generics x extensions interaction faces: type-parameter bounds and
// call-site inference feeding extension resolution, same-name
// extensions on related types (the ambiguity faces), nullable-receiver
// dispatch vs null-aware access, and the explicitly-instantiated
// extension override.
//
// Grounding (upstream Dart SDK source,
// pkg/analyzer/lib/src/dart/resolver/extension_member_resolver.dart):
// implicit dispatch consults only extensions whose instantiated on-type
// is a supertype of the receiver's static type, orders the candidates
// with _isMoreSpecific (instantiated on-type subtyping first, then the
// instantiate-to-bounds tiebreak), and reports
// ambiguous_extension_member_access when neither finalist is more
// specific.
//
// Expected rows: 5 —
//   type_argument_not_matching_bounds, message "'{0}' doesn't conform
//   to the bound '{2}' of the type parameter '{1}'.":
//     the `String` argument of `attach<String>(...)` — misses
//     `T extends Entity`
//   ambiguous_extension_member_access, message "A member named '{0}' is
//   defined in '{1}' and '{2}', and neither is more specific.":
//     `transcript.capacity` — `on Reader` and `on Writer` both apply to
//     `Transcript` and the interfaces are unrelated
//     `samples.total` — `on Iterable<int>` and `on List<num>` both apply
//     to `List<int>` (covariant argument subtyping) and neither on-type
//     is a subtype of the other
//   undefined_getter, message "The getter '{0}' isn't defined for the
//   type '{1}'.":
//     `memo.shouted` — the plain access on `String?` fits no extension;
//     `ShoutExt` is `on String` only, and implicit application reports
//     the plain undefined_* code, not the extension-application one
//   extension_override_argument_not_assignable, message "The type of
//   the argument to the extension override '{0}' isn't assignable to
//   the extended type '{1}'.": `CatalogExt(numbers)` — `List<int>`
//   misses `on List<Book>`

abstract class Entity {
  String get key;
}

class Book implements Entity {
  @override
  final String key;

  Book(this.key);
}

class Repository<T extends Entity> {
  final List<T> items;

  Repository(this.items);
}

// The extension's own type parameter shares the bound, so member
// resolution instantiates through it at every access.
extension RepositoryExt<T extends Entity> on Repository<T> {
  List<String> keys() => [for (var item in items) item.key];
}

// Silence face: inference grounds the extension. `scan(repo)` infers
// T = Book from the argument; both member reads then resolve through
// the inferred subtype Book <: Entity.
T scan<T extends Entity>(Repository<T> repo) => repo.items.first;

extension EntityExt on Entity {
  String get label => 'entity:${key}';
}

void inferredResolution(Repository<Book> repo) {
  var newest = scan(repo);
  print(newest.label);
  print(scan(repo).key);
  print(RepositoryExt(repo).keys());
}

// Bounds face: the explicit type argument is checked against the
// function's bound even though the parameter type itself is unrelated.
Repository<T> attach<T extends Entity>(String path) => Repository([]);

void boundsFace() {
  print(attach<Book>('books.db').items.length);
  // The row: String is not an Entity.
  var loose = attach<String>('notes.db');
  print(loose.items.length);
}

// Ambiguity face A: unrelated interfaces. Transcript implements both,
// each extension declares `capacity`, and neither on-type is a subtype
// of the other, so the access has no more-specific winner.
abstract class Reader {
  List<String> lines();
}

abstract class Writer {
  void write(String line);
}

class Transcript implements Reader, Writer {
  @override
  List<String> lines() => const [];

  @override
  void write(String line) {}
}

extension ReadExt on Reader {
  int get capacity => lines().length;
}

extension WriteExt on Writer {
  int get capacity => 4096;
}

// Ambiguity face B: generic on-types. List<int> is a subtype of both
// Iterable<int> and List<num> (covariant type arguments), but
// Iterable<int> and List<num> are incomparable, so `total` is
// ambiguous while `head`, declared on only one extension, resolves.
extension IntSeqExt on Iterable<int> {
  int get total => reduce((a, b) => a + b);
}

extension NumListExt on List<num> {
  num get head => first;
}

void ambiguityFaces() {
  var transcript = Transcript();
  transcript.write('row');
  print(transcript.capacity);
  var samples = <int>[3, 5, 8];
  print(samples.head);
  // The row: both extensions declare `total`.
  print(samples.total);
}

// Specificity silence faces: the nullable/non-nullable pair resolves by
// on-type subtyping — a String receiver picks the non-nullable on-type
// member (String <: String?, not the reverse), a String? receiver fits
// only the nullable member. The instantiate-to-bounds tiebreak is the
// dual shape for generic extensions: two extensions whose instantiated
// on-types coincide resolve by their bounds (List<num> is a subtype of
// List<dynamic>), so the bounded one wins for List<int>.
extension NullTextExt on String? {
  int get span => 0;
}

extension TextExt on String {
  int get span => length;
}

extension ShoutExt on String {
  String get shouted => toUpperCase();
}

void nullAwareFaces(String? memo, String title) {
  // Null-aware access unwraps to String before dispatch: TextExt wins
  // the specificity order for the unwrapped receiver.
  print(memo?.span);
  // Plain access on String? fits only the nullable on-type member.
  print(memo.span);
  // A non-null receiver resolves the more specific on-String member.
  print(title.span);
  // The unwrapped String applies ShoutExt through the null-aware `?.`.
  print(memo?.shouted);
  // The row: plain access on String? fits no extension at all.
  print(memo.shouted);
}

// Explicit override face: the override argument is checked against the
// declared extended type after full instantiation.
extension CatalogExt on List<Book> {
  List<String> shelfMarks() => const [];
}

void overrideFace(List<int> numbers) {
  print(CatalogExt([]).shelfMarks());
  // The row: List<int> is not assignable to List<Book>.
  print(CatalogExt(numbers).shelfMarks());
}
