// Trigger for dart_code_linter `prefer-using-list-view`.
// Shape: a scrollable `SingleChildScrollView`/`Column` used where a
// `ListView` is preferred — the rule's flagged form.
final scroll = SingleChildScrollView(
  child: Column(children: [Container()]),
);

class SingleChildScrollView extends Widget {
  const SingleChildScrollView({this.child});

  final Widget? child;
}

class Column extends Widget {
  const Column({required this.children});

  final List<Widget> children;
}

class Container extends Widget {
  const Container();
}

class Widget {
  const Widget();
}
