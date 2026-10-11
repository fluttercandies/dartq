// The part target of lib/library_directives.dart. The dotted-name `part
// of` form is the use_string_in_part_of_directives face (the rule
// registers without the unshipped enhanced_parts feature). The syntax
// form fires regardless of part-of resolution on both sides.
// Expected rows: 1 — USE_STRING_IN_PART_OF_DIRECTIVES keyed
// use_string_in_part_of_target.dart.
part of lint_corpus.part_host;

void partHelper() {
  print(4);
}
