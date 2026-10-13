// Targets initializer_for_non_existent_field: a constructor initializer
// naming a field the class does not declare. Oracle message form
// (nail-pinned): "<name> isn't a field in the enclosing class."
class C {
  C() : nope = 0;
}
