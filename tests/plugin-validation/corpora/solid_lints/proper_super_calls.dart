// Trigger for solid_lints `proper_super_calls`.
// Shape: a lifecycle method that forgets the required `super.` call —
// the rule's flagged form.
class _MyState extends State {
  @override
  void initState() {
    counter = 1;
  }

  int counter = 0;
}

class State {
  void initState() {}
}
