// unnecessary_type_name_in_constructor — official rule faces
// (pkg/linter unnecessary_type_name_in_constructor.dart:32-43): every
// constructor declaration reports at its type name — the class-name echo
// the grammar requires before the name part. Factories and the explicit
// `.new` form report at the type name too; the name part never reports.
// Expected rows: 3 — UNNECESSARY_TYPE_NAME_IN_CONSTRUCTOR keyed
// unnecessary_type_name_in_constructor.dart, message
// "Unnecessary type name in a constructor.":
//   the echo `C` of the unnamed constructor
//   the echo `D` of `D.name`
//   the echo `E` of the factory `E.f`
class C {
  C();
}

class D {
  D.name();
}

class E {
  factory E.f() => E._();
  E._();
}
