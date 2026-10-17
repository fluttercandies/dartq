// Trigger for dart_code_linter `avoid-unnecessary-setstate`.
// Shape: `setState` called synchronously inside a lifecycle method where it
// is unnecessary — the rule's flagged form.
class _MyWidgetState extends State {
  @override
  void initState() {
    super.initState();
    setState(() {
      counter = 1;
    });
  }

  int counter = 0;

  void setState(void Function() fn) => fn();
}

class State {
  void initState() {}
}
