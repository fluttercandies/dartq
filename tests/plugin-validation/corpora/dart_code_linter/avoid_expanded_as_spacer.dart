// Trigger for dart_code_linter `avoid-expanded-as-spacer`.
// Shape: `Expanded` wrapping a plain `Container`/`SizedBox` used only as
// spacing — the rule's flagged form.
final column = Column(children: [
  Expanded(child: Container()),
]);

class Column {
  final List<Widget> children;
  const Column({required this.children});
}

class Expanded extends Widget {
  final Widget? child;
  const Expanded({this.child});
}

class Container extends Widget {
  const Container();
}

class Widget {
  const Widget();
}
