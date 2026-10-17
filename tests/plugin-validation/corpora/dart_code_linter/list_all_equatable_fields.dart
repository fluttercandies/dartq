// Trigger for dart_code_linter `list-all-equatable-fields`.
// Shape: an `Equatable` class whose `props` omits a declared field — the
// rule's flagged form.
class Person extends Equatable {
  const Person(this.name, this.age);

  final String name;
  final int age;

  @override
  List<Object?> get props => [name];
}

abstract class Equatable {
  const Equatable();

  List<Object?> get props;
}
