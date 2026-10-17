// Trigger for dart_code_linter `always-remove-listener`.
// Shape: a listener added in `initState` and never removed in `dispose` —
// the rule's flagged form.
class _WidgetState extends State {
  final _listener = Listener();

  @override
  void initState() {
    _listener.addListener(onEvent);
  }

  @override
  void dispose() {
    super.dispose();
  }

  void onEvent() {}
}

class State {
  void initState() {}
  void dispose() {}
}

class Listener {
  void addListener(void Function() listener) {}
}
