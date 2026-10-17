// Trigger for leancode_lint `hook_widget_does_not_use_hooks`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/hook_widget_does_not_use_hooks_test.dart):
// a `HookWidget` subclass whose `build` calls no hook.
class SampleHookWidget extends HookWidget {
  const SampleHookWidget();

  @override
  Widget build(BuildContext context) {
    return Container();
  }
}

class HookWidget extends Widget {
  const HookWidget();
}

class Container extends Widget {
  const Container();
}

class Widget {
  const Widget();
}

class BuildContext {}
