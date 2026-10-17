// Trigger for dart_code_linter
// `avoid-non-configurable-callbacks-in-init-state`.
// Shape: a widget-supplied controller configured in `initState` with
// callbacks that reference no `widget` field — the rule's flagged form.
class MyWidget extends StatefulWidget {
  const MyWidget({required this.controller});

  final WebViewController controller;
}

class _MyWidgetState extends State<MyWidget> {
  @override
  void initState() {
    super.initState();
    widget.controller.setNavigationDelegate(
      NavigationDelegate(onError: (error) {}),
    );
  }
}

class StatefulWidget {
  const StatefulWidget();
}

class State<T> {
  T get widget => throw 0;
  void initState() {}
}

class WebViewController {
  void setNavigationDelegate(NavigationDelegate delegate) {}
}

class NavigationDelegate {
  NavigationDelegate({required this.onError});

  final void Function(Object error) onError;
}
