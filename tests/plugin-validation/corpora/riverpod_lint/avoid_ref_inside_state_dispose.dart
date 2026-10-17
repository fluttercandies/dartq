// Trigger for riverpod_lint `avoid_ref_inside_state_dispose`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/avoid_ref_inside_state_dispose.dart):
// `ref` used inside `State.dispose`.
class _MyState extends ConsumerState<MyWidget> {
  @override
  void dispose() {
    ref.read(someProvider);
    super.dispose();
  }
}

class MyWidget extends ConsumerWidget {
  const MyWidget();
}

class ConsumerWidget {
  const ConsumerWidget();
}

class ConsumerState<T> {
  Ref get ref => Ref();

  void dispose() {}
}

class Ref {
  T read<T>(Provider<T> provider) => provider.value;
}

class Provider<T> {
  Provider(this.value);
  final T value;
}

final someProvider = Provider<int>(0);
