// Trigger for dart_code_linter `provide-correct-intl-args`.
// Shape: an `Intl.message` call whose `args` list mismatches its
// placeholders — the rule's flagged form.
final message = Intl.message('Hello {name}', args: ['unused']);

class Intl {
  static String message(String text, {List<Object>? args}) => text;
}
