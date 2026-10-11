/// Discount pipeline: generic function aliases, a composable pipeline over
/// one type parameter, and a nullable-generic result envelope.
library;

import '../core/money.dart';

/// A pipeline stage over a uniform type.
typedef StageFunction<T> = T Function(T input);

/// Two-argument predicate over candidates.
typedef VerdictPredicate<T> = bool Function(T candidate, T baseline);

/// Pipeline over one type parameter: stages compose left to right.
final class Pipeline<T> {
  final List<StageFunction<T>> _stages;

  Pipeline() : _stages = [];

  const Pipeline.of(this._stages);

  Pipeline<T> append(StageFunction<T> stage) =>
      Pipeline<T>.of([..._stages, stage]);

  T run(T input) {
    var current = input;
    for (final stage in _stages) {
      current = stage(current);
    }
    return current;
  }

  int get stageCount => _stages.length;

  /// Builds a single fused function from the whole pipeline.
  StageFunction<T> fuse() {
    var fused = _stages.fold<StageFunction<T>>(
      (input) => input,
      (chain, stage) => (input) => stage(chain(input)),
    );
    return fused;
  }

  static Pipeline<T> identity<T>() => Pipeline<T>();
}

/// Standard price adjustments, each a reusable tear-off-able function.
/// The ceiling default lives at runtime: extension-type representation
/// constructors are not const, so parameter defaults cannot embed one.
Money capAtCeiling(Money input, {Money? ceiling}) {
  final cap = ceiling ?? Money(500_000);
  return input > cap ? cap : input;
}

Money roundToQuarter(Money input) {
  final remainder = input.cents % 25;
  return remainder == 0 ? input : input + Money(25 - remainder);
}

/// Result envelope over a nullable generic: [value] is null exactly when
/// [error] is non-null.
class PricingOutcome<T extends Object> {
  final T? value;
  final String? error;

  const PricingOutcome.ok(this.value)
      : error = null,
        assert(value != null, 'ok outcomes carry a value');

  const PricingOutcome.failure(this.error)
      : value = null,
        assert(error != null, 'failures carry a message');

  bool get isOk => error == null;

  /// Unwraps with a fallback; the generic nullable default keeps the
  /// envelope usable for non-nullable T.
  T orElse(T fallback) => value ?? fallback;

  /// Maps the success channel; errors pass through untouched.
  PricingOutcome<R> map<R extends Object>(R Function(T value) transform) {
    final current = value;
    if (current == null) {
      return PricingOutcome<R>.failure(error ?? 'unknown');
    }
    return PricingOutcome<R>.ok(transform(current));
  }

  @override
  String toString() => error != null ? 'Outcome(failure: $error)' : 'Outcome(ok: $value)';
}
