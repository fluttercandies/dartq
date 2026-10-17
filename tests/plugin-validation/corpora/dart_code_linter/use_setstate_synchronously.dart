// Trigger for dart_code_linter `use-setstate-synchronously`.
// Shape: `setState` invoked inside an `async` gap/`await` context — the
// rule's flagged form.
class _MyWidgetState extends State {
  Future<void> load() async {
    await fetch();
    setState(() {});
  }

  Future<void> fetch() async {}

  void setState(void Function() fn) => fn();
}

class State {}
