// Trigger for dart_code_linter `avoid-initializing-in-on-mount`.
// Shape: a `late final` field assigned inside `onMount` — the rule's
// flagged form.
class MyComponent extends Component {
  late final int x;

  @override
  void onMount() {
    x = 1;
  }
}

class Component {
  void onMount() {}
}
