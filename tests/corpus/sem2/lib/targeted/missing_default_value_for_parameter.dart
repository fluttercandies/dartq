// Targets missing_default_value_for_parameter: an optional parameter with a
// non-nullable type and no default value has an implicit 'null' default.
// Oracle message (nail-pinned): "The parameter 'x' can't have a value of
// 'null' because of its type, but the implicit default value is 'null'."
void named({int x}) {}

void positional([int y]) {}
