// Trigger for dart_code_linter `avoid-redundant-async-on-load`.
// Shape: `onLoad` marked `async` with no `await` — the rule's flagged form.
import 'dart:async';

class MyComponent extends Component {
  @override
  Future<void> onLoad() async {
    print('hello');
  }
}

class Component {
  FutureOr<void> onLoad() {}
}
