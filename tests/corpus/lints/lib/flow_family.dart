// flow-control family — fixture wave 2 kitchen file. The empty-statement
// faces overlap by construction: the `else ;` empty statement is exactly
// what avoid_empty_else flags, and empty_statements flags every empty
// statement — that file carries both rows (both sides agree on both).
// The bare `;` face keeps the standalone empty_statements row free of
// flow-control shapes (a brace-less `while (c) ;` would co-fire
// curly_braces).
// Expected rows: 5 — AVOID_EMPTY_ELSE keyed flow_family.dart (the else
// `;`), EMPTY_STATEMENTS keyed flow_family.dart (the else `;` plus the
// bare statement: 2 rows, 1 (code, file) pair), EMPTY_CATCHES keyed
// flow_family.dart (the empty catch block, parameter not a bare
// underscore), USE_RETHROW_WHEN_POSSIBLE keyed flow_family.dart (`throw
// error;` of the caught parameter, unshadowed).
void elseUser(bool flag) {
  if (flag) {
    print(0);
  } else ;
}

void bareEmpty() {
  ;
}

void catchUser() {
  try {
    print(1);
  } catch (e) {}
}

void rethrowUser() {
  try {
    print(2);
  } catch (error) {
    throw error;
  }
}
