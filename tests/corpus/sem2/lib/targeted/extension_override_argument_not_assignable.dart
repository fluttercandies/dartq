// Targets extension_override_argument_not_assignable: an explicit
// extension override whose argument is not assignable to the extended
// type. Oracle message (nail-pinned): "The type of the argument to the
// extension override 'String' isn't assignable to the extended type
// 'int'."
extension E on int {}

void f(String s) {
  E(s);
}
