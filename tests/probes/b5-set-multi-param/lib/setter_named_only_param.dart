// Boundary face: setter with a single NAMED parameter.
//
// The official rule checks "exactly one REQUIRED positional parameter",
// not the raw parameter count: `valueFormalParameter.isRequiredPositional`
// is false here, so the rule fires even though the total parameter count
// is 1 (upstream Dart SDK source,
// pkg/analyzer/lib/src/fasta/ast_builder.dart:6504-6511). A raw-count
// check (`last_parameter_count == 1`) would stay silent on this shape,
// so the diff against the oracle distinguishes count-based from
// kind-based enforcement.
class C {
  set named({a}) {}
}
