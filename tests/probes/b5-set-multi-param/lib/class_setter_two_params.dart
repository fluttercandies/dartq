// B5 core face: class-member setter with two parameters.
//
// Official rule: `AstBuilder._ensureSetterFormalParameter` (upstream Dart
// SDK source, pkg/analyzer/lib/src/fasta/ast_builder.dart:6410/:6510)
// reports `wrong_number_of_parameters_for_setter` once, anchored on the
// setter name, because two formals are not exactly one required
// positional parameter. This file pins the row count and code the oracle
// emits for that face.
class C {
  set s(a, b) {}
}
