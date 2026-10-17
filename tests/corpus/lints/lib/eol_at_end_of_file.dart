// eol_at_end_of_file — official rule faces
// (pkg/linter eol_at_end_of_file.dart:36-58): a non-empty file ends in
// exactly one newline — none reports the missing code at the trimmed
// end, several report the too-many code. This fixture carries the
// missing-newline face: the file deliberately ends without a newline.
// Expected rows: 1 — EOL_AT_END_OF_FILE keyed eol_at_end_of_file.dart,
// message "Missing a newline at the end of the file.":
//   the final `}` — the file's last byte, with no terminator after it
class A {}