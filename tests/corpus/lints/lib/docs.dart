// docs family — audit r1 thin row, three doc-comment faces in one file.
// The dangling face must sit on the FIRST declaration of the unit (the
// rule only inspects it): a `///` chain separated from its declaration
// by a blank line. Plain `//` header comments are invisible to all three
// rules (slash_for_doc_comments flags only java-style `/**` docs).
// Expected rows: 3, all keyed docs.dart — DANGLING_LIBRARY_DOC_COMMENTS
// (the first `///` + blank line), SLASH_FOR_DOC_COMMENTS (the `/** */`
// block doc), UNINTENDED_HTML_IN_DOC_COMMENT (`List<int>` in prose,
// outside any code span or fence).
/// A library doc comment left dangling above a blank line.

class DanglingDoced {}

/** A java-style block doc comment that slash_for_doc_comments flags. */
class SlashDoced {}

/// Docs that mention List<int> in prose trip unintended_html_in_doc_comment.
void htmlDoced() {}
