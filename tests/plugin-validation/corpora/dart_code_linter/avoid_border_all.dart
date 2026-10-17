// Trigger for dart_code_linter `avoid-border-all`.
// Shape: `Border.all` invoked with constant arguments — the rule's flagged
// form.
final bad = Container(border: Border.all());

final worse = Container(
  border: Border.all(color: const Color(0), width: 1),
);

class Container {
  final Border? border;

  const Container({this.border});
}

class Border {
  factory Border.all({Color color = const Color(0), double width = 1.0}) =>
      Border._();

  const Border._();
}

class Color {
  const Color(int value);
}
