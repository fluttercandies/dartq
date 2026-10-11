// Targets part_of_name (syntax-codes.tsv:252): with the enhanced-parts
// feature a 'part of' directive can't use a library name. Fasta message
// key partOfName ("The 'part of' directive can't use a name with the
// enhanced-parts feature."), pkg/front_end/lib/src/codes/diagnostic.g.dart.
//
// Zero-exposure face at the corpus language version 3.13: enhanced-parts
// is off by default there (experimental_features.yaml:150, no enabledIn),
// so 'part of foo;' is legal legacy syntax and neither side emits. Kept
// like the native family: pins the legal-legacy shape; the code stays a
// version-gated documented gap until the feature is enabled.
part of foo;
