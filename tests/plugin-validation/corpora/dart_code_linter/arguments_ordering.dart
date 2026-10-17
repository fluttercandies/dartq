// Trigger for dart_code_linter `arguments-ordering`.
// Shape: named arguments passed out of declaration order — the rule's
// flagged form.
Person? createPerson(
    {required String name, required int age, String? surname}) =>
    null;

final badPerson = createPerson(surname: '', name: '', age: 42);

class Person {
  Person({required this.name, required this.age, this.surname});

  final String name;
  final int age;
  final String? surname;
}
