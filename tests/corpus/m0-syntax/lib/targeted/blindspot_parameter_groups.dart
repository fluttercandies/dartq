// Targets mixed_parameter_groups (TSV:224): named and positional groups
// in one parameter list. The rest of this file targets the sibling group
// codes (analyzer ParserErrorCode, pinned snapshot syntax-codes.tsv; the
// analyzer sources are not vendored in this repo).
void mixedGroups(int a, {int b}, [int c]) {}
// Targets multiple_named_parameter_groups (TSV:232): two named groups.
void twoNamedGroups({int a}, {int b}) {}
// Targets multiple_positional_parameter_groups (TSV:235): two positional
// groups.
void twoPositionalGroups([int a], [int b]) {}
// Targets normal_before_optional_parameters (TSV:249): normal parameter
// after the positional group.
void normalAfterOptional([int a], int b) {}
// Targets positional_parameter_outside_group (TSV:256): normal parameter
// after the named group.
void positionalOutsideGroup({int a}, int b) {}
// Targets missing_name_for_named_parameter (TSV:213): a nameless named
// parameter inside a function type.
void namelessNamedParameter(void Function({int}) g) {}
