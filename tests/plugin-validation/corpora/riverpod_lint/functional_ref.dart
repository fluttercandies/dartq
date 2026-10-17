// Trigger for riverpod_lint `functional_ref`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/functional_ref.dart): a
// functional provider whose first parameter is not the matching `ref`
// type.
int counterProvider(int param) => param;

class Ref {
  T watch<T>(Provider<T> provider) => provider.value;
}

class Provider<T> {
  Provider(this.value);
  final T value;
}
