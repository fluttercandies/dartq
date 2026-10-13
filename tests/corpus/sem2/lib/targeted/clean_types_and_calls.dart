// Clean face for the non-class-type and call-shape families (not_a_type,
// non_type_as_type_argument, non_type_in_catch_clause,
// creation_with_non_type, invocation_of_non_function_expression,
// extension_override_argument_not_assignable, extra/not_enough positional
// arguments, list_element_type_not_assignable, invalid_use_of_null_value,
// unchecked_use_of_nullable_value): resolved types, well-argued calls,
// checked nullables; both sides must stay silent.
class C {}

extension E on int {}

void callee(int a) {}

void main() {
  C c = C();
  var l = <int>[1];
  callee(1);
  E(3);
  try {} on FormatException catch (e) {
    print(e);
  }
  bool? maybe = true;
  if (maybe ?? false) print(c);
  print(l.length);
}
