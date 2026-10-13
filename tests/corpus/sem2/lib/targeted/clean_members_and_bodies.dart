// Clean face for the member/body families (conflicting_static_and_instance,
// enum_without_constants, non_final_field_in_enum, body_might_complete_
// normally, non_bool_condition, non_bool_negation_expression,
// non_void_return_for_setter, and the this/super initializer family):
// distinct member names, final enum fields, total bodies; both sides must
// stay silent.
class C {
  static int s() => 0;

  int m() => 1;

  C.ok(int x) : assert(x >= 0);

  set value(int v) {}
}

enum G {
  a,
  b;

  final int code = index;
}

int total() => 1;

void main() {
  var c = C.ok(1);
  c.value = total();
  if (c.m() >= 0 && !(c.m() < 0)) print(c.m() + G.a.code);
}
