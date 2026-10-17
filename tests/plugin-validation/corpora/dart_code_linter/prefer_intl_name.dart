// Trigger for dart_code_linter `prefer-intl-name`.
// Shape: an `Intl.message` call missing a `name` argument — the rule's
// flagged form.
final greeting = Intl.message('Hello');

class Intl {
  static String message(String text, {String? name}) => text;
}
