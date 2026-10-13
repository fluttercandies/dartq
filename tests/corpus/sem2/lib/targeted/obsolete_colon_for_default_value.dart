// Targets obsolete_colon_for_default_value: a named parameter default
// separated by ':' (the pre-2.12 separator). Oracle message (nail-pinned):
// "Using a colon as the separator before a default value is no longer
// supported."
void f({int x: 42}) {}

void g({int y: 7}) {}
