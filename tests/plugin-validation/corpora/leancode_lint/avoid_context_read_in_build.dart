// Trigger for leancode_lint `avoid_context_read_in_build`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/avoid_context_read_in_build_test.dart):
// `context.read` invoked inside a widget `build` method.
class MyWidget extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    context.read<MyCubit>();
    return const SizedBox();
  }
}

class MyCubit {}

class StatelessWidget extends Widget {
  const StatelessWidget();
}

class SizedBox extends Widget {
  const SizedBox();
}

class Widget {
  const Widget();
}

class BuildContext {
  T read<T>() => throw 0;
}
