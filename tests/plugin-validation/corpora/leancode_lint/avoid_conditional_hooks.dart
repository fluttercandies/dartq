// Trigger for leancode_lint `avoid_conditional_hooks`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/avoid_conditional_hooks_test.dart):
// a hook call inside a conditional branch of a widget `build`.
class SampleHookWidget extends HookWidget {
  @override
  Widget build(BuildContext context) {
    if (condition) {
      useState('c');
    }
    return Container();
  }
}

bool condition = true;

Object? useState(Object? value) => value;

class HookWidget extends Widget {}

class Container extends Widget {}

class Widget {
  const Widget();
}

class BuildContext {}
