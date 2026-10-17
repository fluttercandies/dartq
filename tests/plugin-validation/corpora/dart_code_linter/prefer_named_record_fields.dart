// Trigger for dart_code_linter `prefer-named-record-fields`.
// Shape: a record field accessed by positional `$N` where named fields are
// preferred — the rule's flagged form.
(String, int) pair = ('a', 1);

final first = pair.$1;
