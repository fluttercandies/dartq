// switch_on_type — official rule faces
// (pkg/linter switch_on_type.dart:53-121): a switch statement or
// expression whose scrutinee's static type is assignable to `Type`
// reports once at the scrutinee — the leaf faces are a `.runtimeType`
// tail, a bare `Type`-written name, a type parameter, and a type
// literal; the recursion faces walk conditional arms, `+` operands,
// nested switch case values, and interpolation segments.
// Expected rows: 2 — SWITCH_ON_TYPE keyed switch_on_type.dart, message
// "Avoid switch statements on a 'Type'.":
//   the `t` scrutinee (a `Type`-written parameter)
//   the `i.runtimeType` scrutinee
void f(Type t, num i) {
  switch (t) {
    case const (int):
      break;
    default:
      break;
  }
  switch (i.runtimeType) {
    case const (int):
      break;
    default:
      break;
  }
}

// GOOD: a non-Type scrutinee stays silent.
void g(String s) {
  switch (s) {
    case 'a':
      break;
    default:
      break;
  }
}
