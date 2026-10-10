// Targets positional_after_named_argument (syntax-codes.tsv:255): fasta
// message positionalAfterNamedArgument ("Place positional arguments before
// named arguments.", pkg/_fe_analyzer_shared/lib/src/messages/
// diagnostic.g.dart).
void callee(int a, int b) {}

void caller() {
  callee(1, b: 2, 3);
}
