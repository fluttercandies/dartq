// Version-gated error at language version 2.19: records and patterns
// ship at 3.0, so the record type annotation, the record literal, the
// record destructuring pattern and the if-case pattern are all outside
// this file's grammar.
//
// Expectation (oracle at 2.19): the records/patterns shapes are
// rejected below 3.0 — parse/summary errors from the oracle's feature
// gate (LibraryFeatures.records/patterns keyed on language version
// 3.0); exact codes are oracle-pinned and compared like-for-like on
// both sides of the differential.

(int, int) point() => (1, 2);

void main() {
  final (a, b) = point();
  if (point() case (1, _)) {
    print(a + b);
  }
}
