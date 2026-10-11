/// Order validation: generic validators returning record verdicts, with a
/// chain extension over the validator set.
library;

import '../core/identifiers.dart';
import 'package:stock_policy/stock_policy.dart';

/// A validator verdict: positional record (ok, reason?).
typedef Verdict = (bool ok, String? reason);

/// Validation contract; interface class is implemented (never mixed in).
abstract interface class Validator<T> {
  Verdict validate(T candidate);
}

final class SkuShapeValidator implements Validator<SkuId> {
  final int minLength;

  const SkuShapeValidator({this.minLength = 4});

  @override
  Verdict validate(SkuId candidate) {
    if (candidate.value.length < minLength) {
      return (false, 'sku too short: ${candidate.value}');
    }
    return (true, null);
  }
}

final class RestockFloorValidator implements Validator<int> {
  final RestockRule rule;

  const RestockFloorValidator(this.rule);

  @override
  Verdict validate(int onHand) {
    final suggested = rule.evaluate(onHand);
    if (suggested > 0) {
      return (false, 'restock ${suggested}u per ${rule.name}');
    }
    return (true, null);
  }
}

/// Chain extension: all validators must pass; the first failure wins.
extension ValidatorChain<T> on Iterable<Validator<T>> {
  Verdict validateAll(T candidate) {
    for (final validator in this) {
      final (ok, reason) = validator.validate(candidate);
      if (!ok) {
        return (false, reason);
      }
    }
    return (true, null);
  }

  List<Verdict> collectAll(T candidate) =>
      [for (final validator in this) validator.validate(candidate)];
}

/// Facade combining both validators for the intake endpoint.
class IntakeValidator {
  final SkuShapeValidator shape = const SkuShapeValidator();
  final RestockFloorValidator floor;

  IntakeValidator({required RestockRule rule}) : floor = RestockFloorValidator(rule);

  Verdict validateIntake(SkuId sku, int onHand) {
    final (skuOk, skuReason) = shape.validate(sku);
    if (!skuOk) {
      return (false, skuReason);
    }
    return floor.validate(onHand);
  }
}
