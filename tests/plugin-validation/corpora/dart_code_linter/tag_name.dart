// Trigger for dart_code_linter `tag-name`.
// Shape: a `TAG`/`_kTag` constant whose value doesn't match the class
// name — the rule's flagged form.
class Apple extends Fruit {
  static const _kTag = 'Orange';
}

class Fruit {}
