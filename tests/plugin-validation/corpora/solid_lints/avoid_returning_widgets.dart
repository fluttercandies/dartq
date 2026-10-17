// Trigger for solid_lints `avoid_returning_widgets`.
// Shape: a function returning a `Widget` — the rule's flagged form.
Widget buildHeader() => Container();

class Widget {
  const Widget();
}

class Container extends Widget {
  const Container();
}
