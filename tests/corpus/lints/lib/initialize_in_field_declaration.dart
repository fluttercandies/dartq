// initialize_in_field_declaration — official rule faces
// (pkg/linter initialize_in_field_declaration.dart:58-84): a primary
// constructor body initializer `y = expr` whose expression references a
// primary constructor parameter reports at the field name. Late fields
// and fields outside the class skip.
// Expected rows: 2 — INITIALIZE_IN_FIELD_DECLARATION keyed
// initialize_in_field_declaration.dart, message
// "Field should be initialized in the field declaration.":
//   `y` -> the initializer references parameter `x`
//   `w` -> the second initializer in the same body
class C(int x) {
  this : y = x, w = x;
  int y;
  int w;
}

// GOOD: no parameter reference and late fields stay silent.
class D(int x) {
  this : v = 0, l2 = x;
  late int l2;
  int v;
}
