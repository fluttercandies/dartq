// Targets unused_field_from_primary_constructor: a primary-constructor
// declaring formal (`final int _i`) whose synthesized field is never read.
// The class is public and the formal is private, isolating the diagnostic
// to the unused field. Oracle probe (fvm dart 3.13.1,
// `dart analyze --format=machine` over a /tmp copy of this package, one
// row, no noise):
//   WARNING|STATIC_WARNING|UNUSED_FIELD_FROM_PRIMARY_CONSTRUCTOR|
//   lib/targeted/unused_field_from_primary_constructor.dart|10|19|2|
//   The value of the field '_i' isn't used.
class C(final int _i) {}
