// missing_code_block_language_in_doc_comment — official rule faces
// (pkg/linter missing_code_block_language_in_doc_comment.dart:24-58):
// every FENCED doc-comment code block whose info string is missing
// reports at its opening fence line. Indented blocks never report and
// shadow fence parsing while they last; a language-tagged fence passes.
// Expected rows: 1 — MISSING_CODE_BLOCK_LANGUAGE_IN_DOC_COMMENT keyed
// missing_code_block_language_in_doc_comment.dart, message
// "The code block is missing a specified language.":
//   the untagged opening fence of the second block
/// A tagged block:
///
/// ```dart
/// var a = 1;
/// ```
///
/// An untagged block:
///
/// ```
/// var b = 2;
/// ```
class A {}

// GOOD: an indented block is not a fenced face.
///
///     var c = Printer();
///
class B {}
