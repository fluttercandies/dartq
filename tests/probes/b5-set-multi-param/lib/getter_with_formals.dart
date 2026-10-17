// Baseline face: getter with a formal-parameter list.
//
// The official parser reports a recoverable `GetterWithFormals` at the
// open paren (upstream Dart SDK source,
// pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:1689), which
// surfaces to the analyzer as `getter_with_parameters` (SYNTACTIC_ERROR,
// "Getters must be declared without a parameter list."). Both faces
// already agree on this single-row shape, so a clean diff here validates
// the probe machinery end to end while the setter files carry the
// decision delta.
int get g(a) => 1;
