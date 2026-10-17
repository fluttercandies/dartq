// Trigger for dart_code_linter `prefer-define-hero-tag`.
// Shape: a `Hero` widget without an explicit `tag` — the rule's flagged
// form.
final hero = Hero(child: Container());

class Hero extends Widget {
  const Hero({this.tag, this.child});

  final Object? tag;
  final Widget? child;
}

class Container extends Widget {
  const Container();
}

class Widget {
  const Widget();
}
