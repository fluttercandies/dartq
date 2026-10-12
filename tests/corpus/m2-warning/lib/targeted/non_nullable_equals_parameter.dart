// Targets non_nullable_equals_parameter: the '==' operator's parameter
// should be non-nullable (best_practices_verifier.dart,
// _checkForNullableEqualsParameterType). The hash_and_equals INFO face is
// outside the targeted code set.
class P {
  final int x;
  P(this.x);
  @override
  bool operator ==(Object? other) => other is P && other.x == x;
}
