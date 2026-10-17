// Trigger for dart_code_linter `no-boolean-literal-compare`.
// Shape: a boolean compared to a `true`/`false` literal — the rule's
// flagged form.
bool f(bool flag) => flag == true;
