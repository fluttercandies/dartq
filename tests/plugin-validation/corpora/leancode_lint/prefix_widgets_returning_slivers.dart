// Trigger for leancode_lint `prefix_widgets_returning_slivers`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/prefix_widgets_returning_slivers_test.dart):
// a widget whose `build` returns a sliver but whose name lacks the
// configured prefix.
class SliverListWidget extends StatelessWidget {
  const SliverListWidget();

  @override
  Widget build(BuildContext context) {
    return const SliverToBoxAdapter();
  }
}

class StatelessWidget extends Widget {
  const StatelessWidget();
}

class SliverToBoxAdapter extends Widget {
  const SliverToBoxAdapter();
}

class Widget {
  const Widget();
}

class BuildContext {}
