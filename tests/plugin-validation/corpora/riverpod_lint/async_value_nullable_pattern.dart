// Trigger for riverpod_lint `async_value_nullable_pattern`.
// Shaped from the vendored rule source
// (vendor/riverpod_lint-3.1.9/lib/src/lints/async_value_nullable_pattern.dart):
// a null-check pattern destructuring `AsyncValue(:final value?)` on a
// possibly-nullable value.
void f(AsyncValue<int>? asyncValue) {
  switch (asyncValue) {
    case AsyncValue(:final value?):
      print(value);
  }
}

class AsyncValue<T> {
  const AsyncValue({this.value, this.hasValue});

  final T? value;
  final bool? hasValue;
}
