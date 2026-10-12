// Valid Dart 3.15 acceptance corpus — metadata/annotations (const-class and const-instance annotations on library, classes, type parameters, members, constructors, parameters, enum values, typedefs, functions, locals, and loop variables).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/metadata/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
@Traced('on-library-declaration')
library;

const String tag = 'tag';

class Traced {
  final String label;

  const Traced(this.label);
}

const traced = Traced('const');

@traced
@Traced('on-class')
class Annotated<@Traced('on-type-param') T extends Object?> {
  @traced
  static const int threshold = 5;

  @Traced('on-field')
  final T value;

  @Traced('on-constructor')
  const Annotated(this.value);

  @traced
  @Traced('on-method')
  T method(@traced T input, {@Traced('on-named') int step = 1}) => input;

  @Traced('on-getter')
  int get measure => threshold;

  @Traced('on-setter')
  set measure(int v) {}
}

@traced
enum AnnotatedEnum {
  @Traced('on-value')
  first,
  second;

  @traced
  void touch() {}
}

extension AnnotatedExtension<T> on Annotated<T> {
  @traced
  T get again => value;
}

@traced
typedef AnnotatedAlias = void Function(@Traced('on-param') int code);

@Traced('on-function')
void annotatedFunction() {
  @traced
  final local = 1;
  for (@Traced('on-loop-var') var i = 0; i < local; i++) {}
}

void main() {
  final a = Annotated<int>(3);
  a.measure = a.measure;
  a.method(4);
  a.again;
  AnnotatedEnum.first.touch();
  annotatedFunction();
  final items = <Object>[a, a.value, a.measure, AnnotatedEnum.second, tag];
  if (items.length != 5) {
    throw StateError('unreachable');
  }
}
