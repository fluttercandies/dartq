// Trigger for leancode_lint `avoid_single_child_in_multi_child_widgets`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/avoid_single_child_in_multi_child_widget_test.dart):
// a multi-child widget given a single child.
Widget test() {
  return Column(
    children: [Container()],
  );
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
