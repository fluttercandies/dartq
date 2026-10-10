// Targets encoding (syntax-codes.tsv:90): the scanner reports a source
// file whose bytes do not decode as UTF-8 ("Unable to decode bytes as
// UTF-8.", pkg/_fe_analyzer_shared/lib/src/messages/diagnostic.g.dart,
// codeEncoding). The byte below is a deliberate invalid UTF-8 sequence
// inside a comment.
// invalid: ÿþ
void f() {}
