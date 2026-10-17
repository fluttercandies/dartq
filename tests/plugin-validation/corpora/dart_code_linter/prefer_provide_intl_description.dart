// Trigger for dart_code_linter `prefer-provide-intl-description`.
// Shape: an `Intl.message` call missing a `desc` argument — the rule's
// flagged form.
final greeting = Intl.message('Hello', name: 'greeting');

class Intl {
  static String message(String text, {String? name, String? desc}) => text;
}
