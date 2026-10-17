// Trigger for solid_lints `avoid_unnecessary_setstate`.
// Shape: `setState` invoked where a direct mutation suffices — the rule's
// flagged form.
class _MyState extends State {
  void onTap() {
    setState(() {
      counter = 1;
    });
  }

  int counter = 0;

  void setState(void Function() fn) => fn();
}

class State {}
