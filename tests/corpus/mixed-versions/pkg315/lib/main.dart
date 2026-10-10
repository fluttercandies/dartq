// Future-version anticipation package: pins language version 3.15 while
// using ONLY shapes valid at 3.13 (no enable-experiment content anywhere
// in this corpus — that is a separate corpus gap).
//
// Expectation split: a 3.13 oracle CLAMPS the future package version to
// its supported maximum; a 3.15-aware engine resolves it natively. Either
// way the file's shapes are legal — this is a version-resolution face,
// not a feature gate.

sealed class Result<T> {}

final class Ok<T> extends Result<T> {
  final T value;

  Ok(this.value);
}

final class Err<T> extends Result<T> {
  final String message;

  Err(this.message);
}

extension type Rank(int level) {}

const int ceiling = 10_000;

String render<T>(Result<T> r) {
  return switch (r) {
    Ok(value: var v) => 'ok $v',
    Err(message: var m) => 'err $m',
  };
}

void main() {
  print(render(Ok(Rank(ceiling).value)));
}
