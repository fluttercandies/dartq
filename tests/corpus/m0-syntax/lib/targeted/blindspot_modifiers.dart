// Targets the member-modifier family (analyzer ParserErrorCode, pinned
// snapshot syntax-codes.tsv; the analyzer sources are not vendored in
// this repo).
// Targets abstract_static_method (TSV:52).
class AbstractStaticMethod {
  abstract static void m();
}

// Targets final_method (TSV:140).
class FinalMethod {
  final void m() {}
}

// Targets covariant_constructor (TSV:76).
class CovariantConstructor {
  covariant CovariantConstructor();
}

// Targets final_constructor (TSV:138).
class FinalConstructorDecl {
  final FinalConstructorDecl();
}

// Targets local_function_declaration_modifier (TSV:193).
void localModifier() {
  static void g() {}
}
