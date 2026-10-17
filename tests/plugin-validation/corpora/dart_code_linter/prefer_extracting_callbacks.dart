// Trigger for dart_code_linter `prefer-extracting-callbacks`.
// Shape: an inline closure passed as a widget callback — the rule's
// flagged form.
final button = ElevatedButton(
  onPressed: () {
    print('pressed');
  },
);

class ElevatedButton extends Widget {
  const ElevatedButton({required this.onPressed});

  final void Function() onPressed;
}

class Widget {
  const Widget();
}
