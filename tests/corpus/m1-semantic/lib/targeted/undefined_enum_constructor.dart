// Targets undefined_enum_constructor: an enum constant's constructor
// selector names a constructor the enum does not declare (the
// enum-constant-initializer emission path; a plain `E.named()` invocation
// reports undefined_method instead on dart 3.13.1).
enum E {
  c.other();

  const E();
}
